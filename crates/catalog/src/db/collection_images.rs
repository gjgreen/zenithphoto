use crate::db::{parse_datetime, query_all, query_one, to_rfc3339, DbHandle, DbResult};
use anyhow::Context;
use chrono::{DateTime, Utc};
use rusqlite::params;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionImage {
    pub collection_id: i64,
    pub image_id: i64,
    pub position: i64,
    pub added_at: DateTime<Utc>,
}

impl CollectionImage {
    pub fn insert<H: DbHandle>(&self, db: &H) -> DbResult<i64> {
        db.execute(
            "INSERT INTO collection_images (collection_id, image_id, position, added_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                self.collection_id,
                self.image_id,
                self.position,
                to_rfc3339(self.added_at)
            ],
        )
        .with_context(|| {
            format!(
                "failed to insert collection_image collection_id={} image_id={}",
                self.collection_id, self.image_id
            )
        })?;
        Ok(db.last_insert_rowid())
    }

    pub fn load<H: DbHandle>(db: &H, collection_id: i64, image_id: i64) -> DbResult<Self> {
        query_one(
            db,
            "SELECT collection_id, image_id, position, added_at
             FROM collection_images
             WHERE collection_id = ?1 AND image_id = ?2",
            params![collection_id, image_id],
            CollectionImage::from_row,
        )
        .with_context(|| {
            format!(
                "failed to load collection_image collection_id={} image_id={}",
                collection_id, image_id
            )
        })
    }

    pub fn load_all<H: DbHandle>(db: &H) -> DbResult<Vec<Self>> {
        query_all(
            db,
            "SELECT collection_id, image_id, position, added_at FROM collection_images ORDER BY added_at DESC",
            [],
            CollectionImage::from_row,
        )
    }

    pub fn update<H: DbHandle>(&self, db: &H) -> DbResult<()> {
        db.execute(
            "UPDATE collection_images
             SET position = ?1, added_at = ?2
             WHERE collection_id = ?3 AND image_id = ?4",
            params![
                self.position,
                to_rfc3339(self.added_at),
                self.collection_id,
                self.image_id
            ],
        )
        .with_context(|| {
            format!(
                "failed to update collection_image collection_id={} image_id={}",
                self.collection_id, self.image_id
            )
        })?;
        Ok(())
    }

    pub fn delete<H: DbHandle>(db: &H, collection_id: i64, image_id: i64) -> DbResult<()> {
        db.execute(
            "DELETE FROM collection_images WHERE collection_id = ?1 AND image_id = ?2",
            params![collection_id, image_id],
        )
        .with_context(|| {
            format!(
                "failed to delete collection_image collection_id={} image_id={}",
                collection_id, image_id
            )
        })?;
        Ok(())
    }

    fn from_row(row: &rusqlite::Row<'_>) -> DbResult<Self> {
        Ok(Self {
            collection_id: row.get(0)?,
            image_id: row.get(1)?,
            position: row.get(2)?,
            added_at: parse_datetime(row.get::<_, String>(3)?, "added_at")?,
        })
    }
}
