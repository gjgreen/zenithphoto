//! ORM-style bindings for the catalog SQLite schema.

use anyhow::Context;
use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{Connection, Row, Transaction};
use serde::de::DeserializeOwned;
use serde::Serialize;

pub mod catalog_metadata;
pub mod collection_images;
pub mod collections;
pub mod db;
pub mod edit_history;
pub mod edits;
pub mod folders;
pub mod image_keywords;
pub mod images;
pub mod keywords;
pub mod migrations;
pub mod previews;
pub mod thumbnails;

pub use catalog_metadata::CatalogMetadata;
pub use collection_images::CollectionImage;
pub use collections::Collection;
pub use db::CatalogDb;
pub use edit_history::EditHistory;
pub use edits::Edit;
pub use folders::Folder;
pub use image_keywords::ImageKeyword;
pub use images::Image;
pub use keywords::Keyword;
pub use migrations::{Migration, MIGRATIONS};
pub use previews::Preview;
pub use thumbnails::Thumbnail;

pub type DbResult<T> = anyhow::Result<T>;

/// Common trait allowing modules to operate over either a `Connection` or `Transaction`.
pub trait DbHandle {
    fn execute(&self, sql: &str, params: impl rusqlite::Params) -> rusqlite::Result<usize>;
    fn prepare<'a>(&'a self, sql: &str) -> rusqlite::Result<rusqlite::Statement<'a>>;
    fn last_insert_rowid(&self) -> i64;

    fn query_row<T>(
        &self,
        sql: &str,
        params: impl rusqlite::Params,
        f: impl FnOnce(&Row) -> DbResult<T>,
    ) -> DbResult<T>
    where
        Self: Sized,
    {
        query_one(self, sql, params, f)
    }
}

impl DbHandle for Connection {
    fn execute(&self, sql: &str, params: impl rusqlite::Params) -> rusqlite::Result<usize> {
        Connection::execute(self, sql, params)
    }

    fn prepare<'a>(&'a self, sql: &str) -> rusqlite::Result<rusqlite::Statement<'a>> {
        Connection::prepare(self, sql)
    }

    fn last_insert_rowid(&self) -> i64 {
        Connection::last_insert_rowid(self)
    }
}

impl<'a> DbHandle for Transaction<'a> {
    fn execute(&self, sql: &str, params: impl rusqlite::Params) -> rusqlite::Result<usize> {
        (**self).execute(sql, params)
    }

    fn prepare<'a_stmt>(
        &'a_stmt self,
        sql: &str,
    ) -> rusqlite::Result<rusqlite::Statement<'a_stmt>> {
        (**self).prepare(sql)
    }

    fn last_insert_rowid(&self) -> i64 {
        (**self).last_insert_rowid()
    }
}

/// Map a single row result to a typed value, returning an error when no rows are present.
pub fn query_one<T, H, P, F>(db: &H, sql: &str, params: P, map: F) -> DbResult<T>
where
    H: DbHandle + ?Sized,
    P: rusqlite::Params,
    F: FnOnce(&Row) -> DbResult<T>,
{
    let mut stmt = db.prepare(sql)?;
    let mut rows = stmt.query(params)?;
    let row = rows.next()?.context("query returned no rows")?;
    map(&row)
}

/// Map at most one row result to a typed value.
pub fn query_optional<T, H, P, F>(db: &H, sql: &str, params: P, mut map: F) -> DbResult<Option<T>>
where
    H: DbHandle + ?Sized,
    P: rusqlite::Params,
    F: FnMut(&Row) -> DbResult<T>,
{
    let mut stmt = db.prepare(sql)?;
    let mut rows = stmt.query(params)?;
    match rows.next()? {
        Some(row) => Ok(Some(map(&row)?)),
        None => Ok(None),
    }
}

/// Collect all rows from a query into a vector.
pub fn query_all<T, H, P, F>(db: &H, sql: &str, params: P, mut map: F) -> DbResult<Vec<T>>
where
    H: DbHandle + ?Sized,
    P: rusqlite::Params,
    F: FnMut(&Row) -> DbResult<T>,
{
    let mut stmt = db.prepare(sql)?;
    let mut rows = stmt.query(params)?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(map(&row)?);
    }
    Ok(out)
}

pub fn to_rfc3339(ts: DateTime<Utc>) -> String {
    ts.to_rfc3339_opts(SecondsFormat::Millis, true)
}

pub fn to_rfc3339_opt(ts: Option<DateTime<Utc>>) -> Option<String> {
    ts.map(to_rfc3339)
}

pub fn parse_datetime(raw: String, field: &str) -> DbResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(&raw)
        .map(|dt| dt.with_timezone(&Utc))
        .with_context(|| format!("failed to parse {field} timestamp: {raw}"))
}

pub fn parse_datetime_opt(raw: Option<String>, field: &str) -> DbResult<Option<DateTime<Utc>>> {
    raw.map(|value| parse_datetime(value, field)).transpose()
}

pub fn to_json<T: Serialize>(value: &T) -> DbResult<String> {
    serde_json::to_string(value).context("failed to serialize JSON column")
}

pub fn from_json<T: DeserializeOwned>(s: &str) -> DbResult<T> {
    serde_json::from_str(s).context("failed to deserialize JSON column")
}
