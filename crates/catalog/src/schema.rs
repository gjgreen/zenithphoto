//! Modern SQLite schema DDL and helper utilities for catalog initialization.

use anyhow::{anyhow, Context, Result};
use rusqlite::{Connection, OptionalExtension};

use crate::db::migrations::{self, MIGRATIONS};

/// SQLite schema version supported by this build.
pub const TARGET_SCHEMA_VERSION: i64 = migrations::LATEST_SCHEMA_VERSION as i64;

/// Packed SQL definition for the complete catalog schema.
pub const CATALOG_SCHEMA_SQL: &str = include_str!("../schema/catalog_schema.sql");

/// Applies the catalog schema (or upgrades an existing catalog) on the provided connection.
///
/// The helper enforces WAL journaling + foreign keys, runs any pending migrations,
/// ensures the `catalog_metadata` row exists, and keeps `PRAGMA user_version`
/// aligned with the Rust-side [`TARGET_SCHEMA_VERSION`].
pub fn initialize_schema(conn: &Connection) -> Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")
        .context("failed to set journal_mode=WAL")?;
    conn.pragma_update(None, "foreign_keys", true)
        .context("failed to enable foreign_keys")?;

    let has_metadata_table: bool = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'catalog_metadata'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .is_some();

    if !has_metadata_table {
        conn.execute_batch(CATALOG_SCHEMA_SQL)
            .context("failed to apply base catalog schema")?;
    }

    let current_version = migrations::current_schema_version_for_conn(conn)?;
    if current_version == 0 {
        // Ensure a baseline row exists without overwriting existing versioned catalogs.
        migrations::set_schema_version_for_conn(conn, 1)?;
    }

    let current_version = migrations::current_schema_version_for_conn(conn)?;
    if current_version > TARGET_SCHEMA_VERSION as i32 {
        return Err(anyhow!(
            "catalog schema version {} is newer than supported {}",
            current_version,
            TARGET_SCHEMA_VERSION
        ));
    }

    migrations::run_migrations_for_conn(conn, MIGRATIONS)
        .context("failed to run catalog migrations")?;

    conn.execute(
        "UPDATE catalog_metadata SET last_opened = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id = 1",
        [],
    )?;

    Ok(())
}
