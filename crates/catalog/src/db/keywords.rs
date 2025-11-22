use crate::db::{query_all, query_one, DbHandle, DbResult};
use anyhow::Context;
use rusqlite::params;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Keyword {
    pub id: i64,
    pub keyword: String,
}

impl Keyword {
    pub fn insert<H: DbHandle>(&self, db: &H) -> DbResult<i64> {
        db.execute(
            "INSERT INTO keywords (keyword) VALUES (?1)",
            params![self.keyword],
        )
        .with_context(|| format!("failed to insert keyword {}", self.keyword))?;
        Ok(db.last_insert_rowid())
    }

    pub fn load<H: DbHandle>(db: &H, id: i64) -> DbResult<Self> {
        query_one(
            db,
            "SELECT id, keyword FROM keywords WHERE id = ?1",
            params![id],
            Keyword::from_row,
        )
        .with_context(|| format!("failed to load keyword id={id}"))
    }

    pub fn load_all<H: DbHandle>(db: &H) -> DbResult<Vec<Self>> {
        query_all(
            db,
            "SELECT id, keyword FROM keywords ORDER BY keyword",
            [],
            Keyword::from_row,
        )
    }

    pub fn update<H: DbHandle>(&self, db: &H) -> DbResult<()> {
        db.execute(
            "UPDATE keywords SET keyword = ?1 WHERE id = ?2",
            params![self.keyword, self.id],
        )
        .with_context(|| format!("failed to update keyword id={}", self.id))?;
        Ok(())
    }

    pub fn delete<H: DbHandle>(db: &H, id: i64) -> DbResult<()> {
        db.execute("DELETE FROM keywords WHERE id = ?1", params![id])
            .with_context(|| format!("failed to delete keyword id={id}"))?;
        Ok(())
    }

    pub fn get_or_create<H: DbHandle>(db: &H, keyword: &str) -> DbResult<Self> {
        if let Some(existing) = crate::db::query_optional(
            db,
            "SELECT id, keyword FROM keywords WHERE keyword = ?1",
            params![keyword],
            Keyword::from_row,
        )? {
            return Ok(existing);
        }

        db.execute(
            "INSERT OR IGNORE INTO keywords (keyword) VALUES (?1)",
            params![keyword],
        )
        .with_context(|| format!("failed to insert keyword {}", keyword))?;
        query_one(
            db,
            "SELECT id, keyword FROM keywords WHERE keyword = ?1",
            params![keyword],
            Keyword::from_row,
        )
    }

    pub(crate) fn from_row(row: &rusqlite::Row<'_>) -> DbResult<Self> {
        Ok(Self {
            id: row.get(0)?,
            keyword: row.get(1)?,
        })
    }
}
