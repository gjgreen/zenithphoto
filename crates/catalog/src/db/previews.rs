use crate::db::{parse_datetime, query_all, query_one, to_rfc3339, DbHandle, DbResult};
use anyhow::Context;
use chrono::{DateTime, Utc};
use rusqlite::params;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Preview {
    pub image_id: i64,
    pub preview_blob: Option<Vec<u8>>,
    pub updated_at: DateTime<Utc>,
}

impl Preview {
    pub fn insert<H: DbHandle>(&self, db: &H) -> DbResult<i64> {
        db.execute(
            "INSERT INTO previews (image_id, preview_blob, updated_at)
             VALUES (?1, ?2, ?3)",
            params![
                self.image_id,
                self.preview_blob.as_ref(),
                to_rfc3339(self.updated_at)
            ],
        )
        .with_context(|| format!("failed to insert preview for image_id={}", self.image_id))?;
        Ok(db.last_insert_rowid())
    }

    pub fn load<H: DbHandle>(db: &H, image_id: i64) -> DbResult<Self> {
        query_one(
            db,
            "SELECT image_id, preview_blob, updated_at FROM previews WHERE image_id = ?1",
            params![image_id],
            Preview::from_row,
        )
        .with_context(|| format!("failed to load preview for image_id={image_id}"))
    }

    pub fn load_all<H: DbHandle>(db: &H) -> DbResult<Vec<Self>> {
        query_all(
            db,
            "SELECT image_id, preview_blob, updated_at FROM previews",
            [],
            Preview::from_row,
        )
    }

    pub fn update<H: DbHandle>(&self, db: &H) -> DbResult<()> {
        db.execute(
            "UPDATE previews SET preview_blob = ?1, updated_at = ?2 WHERE image_id = ?3",
            params![
                self.preview_blob.as_ref(),
                to_rfc3339(self.updated_at),
                self.image_id
            ],
        )
        .with_context(|| format!("failed to update preview for image_id={}", self.image_id))?;
        Ok(())
    }

    pub fn delete<H: DbHandle>(db: &H, image_id: i64) -> DbResult<()> {
        db.execute(
            "DELETE FROM previews WHERE image_id = ?1",
            params![image_id],
        )
        .with_context(|| format!("failed to delete preview for image_id={image_id}"))?;
        Ok(())
    }

    fn from_row(row: &rusqlite::Row<'_>) -> DbResult<Self> {
        Ok(Self {
            image_id: row.get(0)?,
            preview_blob: row.get(1)?,
            updated_at: parse_datetime(row.get::<_, String>(2)?, "updated_at")?,
        })
    }
}
