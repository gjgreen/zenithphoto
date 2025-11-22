use crate::db::{parse_datetime, query_all, query_one, to_rfc3339, DbHandle, DbResult};
use anyhow::Context;
use chrono::{DateTime, Utc};
use rusqlite::params;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Thumbnail {
    pub image_id: i64,
    pub thumb_256: Option<Vec<u8>>,
    pub thumb_1024: Option<Vec<u8>>,
    pub updated_at: DateTime<Utc>,
}

impl Thumbnail {
    pub fn insert<H: DbHandle>(&self, db: &H) -> DbResult<i64> {
        db.execute(
            "INSERT INTO thumbnails (image_id, thumb_256, thumb_1024, updated_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                self.image_id,
                self.thumb_256.as_ref(),
                self.thumb_1024.as_ref(),
                to_rfc3339(self.updated_at)
            ],
        )
        .with_context(|| format!("failed to insert thumbnail for image_id={}", self.image_id))?;
        Ok(db.last_insert_rowid())
    }

    pub fn load<H: DbHandle>(db: &H, image_id: i64) -> DbResult<Self> {
        query_one(
            db,
            "SELECT image_id, thumb_256, thumb_1024, updated_at FROM thumbnails WHERE image_id = ?1",
            params![image_id],
            Thumbnail::from_row,
        )
        .with_context(|| format!("failed to load thumbnail for image_id={image_id}"))
    }

    pub fn load_all<H: DbHandle>(db: &H) -> DbResult<Vec<Self>> {
        query_all(
            db,
            "SELECT image_id, thumb_256, thumb_1024, updated_at FROM thumbnails",
            [],
            Thumbnail::from_row,
        )
    }

    pub fn update<H: DbHandle>(&self, db: &H) -> DbResult<()> {
        db.execute(
            "UPDATE thumbnails
             SET thumb_256 = ?1, thumb_1024 = ?2, updated_at = ?3
             WHERE image_id = ?4",
            params![
                self.thumb_256.as_ref(),
                self.thumb_1024.as_ref(),
                to_rfc3339(self.updated_at),
                self.image_id
            ],
        )
        .with_context(|| format!("failed to update thumbnail for image_id={}", self.image_id))?;
        Ok(())
    }

    pub fn delete<H: DbHandle>(db: &H, image_id: i64) -> DbResult<()> {
        db.execute(
            "DELETE FROM thumbnails WHERE image_id = ?1",
            params![image_id],
        )
        .with_context(|| format!("failed to delete thumbnail for image_id={image_id}"))?;
        Ok(())
    }

    fn from_row(row: &rusqlite::Row<'_>) -> DbResult<Self> {
        Ok(Self {
            image_id: row.get(0)?,
            thumb_256: row.get(1)?,
            thumb_1024: row.get(2)?,
            updated_at: parse_datetime(row.get::<_, String>(3)?, "updated_at")?,
        })
    }
}
