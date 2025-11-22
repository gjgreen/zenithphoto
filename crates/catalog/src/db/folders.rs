use crate::db::{
    parse_datetime, query_all, query_one, query_optional, to_rfc3339, DbHandle, DbResult,
};
use anyhow::Context;
use chrono::{DateTime, Utc};
use rusqlite::params;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Folder {
    pub id: i64,
    pub path: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Folder {
    pub fn insert<H: DbHandle>(&self, db: &H) -> DbResult<i64> {
        db.execute(
            "INSERT INTO folders (path, created_at, updated_at) VALUES (?1, ?2, ?3)",
            params![
                self.path,
                to_rfc3339(self.created_at),
                to_rfc3339(self.updated_at)
            ],
        )
        .with_context(|| format!("failed to insert folder path={}", self.path))?;
        Ok(db.last_insert_rowid())
    }

    pub fn load<H: DbHandle>(db: &H, id: i64) -> DbResult<Self> {
        query_one(
            db,
            "SELECT id, path, created_at, updated_at FROM folders WHERE id = ?1",
            params![id],
            Folder::from_row,
        )
        .with_context(|| format!("failed to load folder id={id}"))
    }

    pub fn load_all<H: DbHandle>(db: &H) -> DbResult<Vec<Self>> {
        query_all(
            db,
            "SELECT id, path, created_at, updated_at FROM folders ORDER BY id",
            [],
            Folder::from_row,
        )
    }

    pub fn update<H: DbHandle>(&self, db: &H) -> DbResult<()> {
        db.execute(
            "UPDATE folders SET path = ?1, created_at = ?2, updated_at = ?3 WHERE id = ?4",
            params![
                self.path,
                to_rfc3339(self.created_at),
                to_rfc3339(self.updated_at),
                self.id
            ],
        )
        .with_context(|| format!("failed to update folder id={}", self.id))?;
        Ok(())
    }

    pub fn delete<H: DbHandle>(db: &H, id: i64) -> DbResult<()> {
        db.execute("DELETE FROM folders WHERE id = ?1", params![id])
            .with_context(|| format!("failed to delete folder id={id}"))?;
        Ok(())
    }

    pub fn find_by_path<H: DbHandle>(db: &H, path: &str) -> DbResult<Option<Self>> {
        query_optional(
            db,
            "SELECT id, path, created_at, updated_at FROM folders WHERE path = ?1",
            params![path],
            Folder::from_row,
        )
    }

    pub(crate) fn from_row(row: &rusqlite::Row<'_>) -> DbResult<Self> {
        Ok(Self {
            id: row.get(0)?,
            path: row.get(1)?,
            created_at: parse_datetime(row.get::<_, String>(2)?, "created_at")?,
            updated_at: parse_datetime(row.get::<_, String>(3)?, "updated_at")?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::CatalogDb;
    use crate::schema::initialize_schema;

    #[test]
    fn find_by_path_round_trip() {
        let db = CatalogDb::in_memory().unwrap();
        initialize_schema(db.conn()).unwrap();

        let folder = Folder {
            id: 0,
            path: "/photos/2024".into(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        folder.insert(&db).unwrap();

        let fetched = Folder::find_by_path(&db, "/photos/2024")
            .unwrap()
            .expect("folder not found");
        assert_eq!(fetched.path, folder.path);
    }

    #[test]
    fn insert_with_transaction_handle() {
        let mut db = CatalogDb::in_memory().unwrap();
        initialize_schema(db.conn()).unwrap();

        let folder = Folder {
            id: 0,
            path: "/tmp".into(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        let tx = db.transaction().unwrap();
        let folder_id = folder.insert(&tx).unwrap();
        tx.commit().unwrap();

        let loaded = Folder::load(&db, folder_id).unwrap();
        assert_eq!(loaded.path, "/tmp");
    }
}
