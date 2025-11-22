use crate::db::{
    parse_datetime, parse_datetime_opt, query_all, query_one, query_optional, to_rfc3339,
    to_rfc3339_opt, DbHandle, DbResult,
};
use anyhow::Context;
use chrono::{DateTime, Utc};
use rusqlite::params;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogMetadata {
    pub id: i64,
    pub schema_version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_opened: Option<DateTime<Utc>>,
}

impl CatalogMetadata {
    pub fn insert<H: DbHandle>(&self, db: &H) -> DbResult<i64> {
        db.execute(
            "INSERT INTO catalog_metadata (id, schema_version, created_at, updated_at, last_opened)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                self.id,
                self.schema_version,
                to_rfc3339(self.created_at),
                to_rfc3339(self.updated_at),
                to_rfc3339_opt(self.last_opened)
            ],
        )
        .with_context(|| format!("failed to insert catalog_metadata id={}", self.id))?;
        Ok(db.last_insert_rowid())
    }

    pub fn load<H: DbHandle>(db: &H, id: i64) -> DbResult<Self> {
        query_one(
            db,
            "SELECT id, schema_version, created_at, updated_at, last_opened
             FROM catalog_metadata WHERE id = ?1",
            params![id],
            CatalogMetadata::from_row,
        )
        .with_context(|| format!("failed to load catalog_metadata id={id}"))
    }

    pub fn load_all<H: DbHandle>(db: &H) -> DbResult<Vec<Self>> {
        query_all(
            db,
            "SELECT id, schema_version, created_at, updated_at, last_opened
             FROM catalog_metadata ORDER BY id",
            [],
            CatalogMetadata::from_row,
        )
    }

    pub fn update<H: DbHandle>(&self, db: &H) -> DbResult<()> {
        db.execute(
            "UPDATE catalog_metadata
             SET schema_version = ?1, created_at = ?2, updated_at = ?3, last_opened = ?4
             WHERE id = ?5",
            params![
                self.schema_version,
                to_rfc3339(self.created_at),
                to_rfc3339(self.updated_at),
                to_rfc3339_opt(self.last_opened),
                self.id
            ],
        )
        .with_context(|| format!("failed to update catalog_metadata id={}", self.id))?;
        Ok(())
    }

    pub fn delete<H: DbHandle>(db: &H, id: i64) -> DbResult<()> {
        db.execute("DELETE FROM catalog_metadata WHERE id = ?1", params![id])
            .with_context(|| format!("failed to delete catalog_metadata id={id}"))?;
        Ok(())
    }

    pub fn load_singleton<H: DbHandle>(db: &H) -> DbResult<Self> {
        query_optional(
            db,
            "SELECT id, schema_version, created_at, updated_at, last_opened
             FROM catalog_metadata WHERE id = 1",
            [],
            CatalogMetadata::from_row,
        )?
        .with_context(|| "catalog_metadata singleton row is missing".to_string())
    }

    pub fn update_last_opened<H: DbHandle>(db: &H) -> DbResult<()> {
        let now = Utc::now();
        db.execute(
            "UPDATE catalog_metadata SET last_opened = ?1 WHERE id = 1",
            params![to_rfc3339(now)],
        )
        .with_context(|| "failed to update catalog_metadata.last_opened")?;
        Ok(())
    }

    fn from_row(row: &rusqlite::Row<'_>) -> DbResult<Self> {
        Ok(Self {
            id: row.get(0)?,
            schema_version: row.get(1)?,
            created_at: parse_datetime(row.get::<_, String>(2)?, "created_at")?,
            updated_at: parse_datetime(row.get::<_, String>(3)?, "updated_at")?,
            last_opened: parse_datetime_opt(row.get::<_, Option<String>>(4)?, "last_opened")?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::CatalogDb;
    use crate::schema::initialize_schema;

    #[test]
    fn insert_and_round_trip_singleton() {
        let db = CatalogDb::in_memory().unwrap();
        initialize_schema(db.conn()).unwrap();

        let fetched = CatalogMetadata::load_singleton(&db).unwrap();
        assert_eq!(fetched.id, 1);

        CatalogMetadata::update_last_opened(&db).unwrap();
        let updated = CatalogMetadata::load_singleton(&db).unwrap();
        assert!(updated.last_opened.is_some());
    }
}
