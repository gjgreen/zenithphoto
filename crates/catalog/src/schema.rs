//! Modern SQLite schema DDL and helper utilities for catalog initialization.

use rusqlite::{Connection, Error as SqliteError, ErrorCode, OptionalExtension};

/// SQLite schema version supported by this build.
pub const TARGET_SCHEMA_VERSION: i64 = 1;

/// Packed SQL definition for the complete catalog schema.
pub const CATALOG_SCHEMA_SQL: &str = include_str!("../schema/catalog_schema.sql");

/// Applies the catalog schema (or upgrades an existing catalog) on the provided connection.
///
/// The helper enforces WAL journaling + foreign keys, runs any pending migrations,
/// ensures the `catalog_metadata` row exists, and keeps `PRAGMA user_version`
/// aligned with the Rust-side [`TARGET_SCHEMA_VERSION`].
pub fn initialize_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", true)?;

    let user_version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if user_version > TARGET_SCHEMA_VERSION {
        return Err(newer_schema_error(user_version));
    }

    if user_version == 0 || user_version < TARGET_SCHEMA_VERSION {
        apply_migrations(conn, user_version)?;
    }

    // Check the application-level schema tracking row (if it exists).
    let current_catalog_version: i64 = conn
        .query_row(
            "SELECT schema_version FROM catalog_metadata WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or(0);

    if current_catalog_version > TARGET_SCHEMA_VERSION {
        return Err(newer_schema_error(current_catalog_version));
    }
    if current_catalog_version < TARGET_SCHEMA_VERSION {
        apply_migrations(conn, current_catalog_version)?;
    }

    conn.execute(
        "INSERT INTO catalog_metadata (id, schema_version, created_at, updated_at, last_opened)
         VALUES (
            1,
            ?1,
            strftime('%Y-%m-%dT%H:%M:%fZ','now'),
            strftime('%Y-%m-%dT%H:%M:%fZ','now'),
            NULL
         )
         ON CONFLICT(id) DO UPDATE SET
            schema_version = excluded.schema_version,
            updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')",
        [TARGET_SCHEMA_VERSION],
    )?;

    conn.pragma_update(None, "user_version", &TARGET_SCHEMA_VERSION)?;
    Ok(())
}

fn apply_migrations(conn: &Connection, from_version: i64) -> rusqlite::Result<()> {
    match from_version {
        0 => {
            conn.execute_batch(CATALOG_SCHEMA_SQL)?;
        }
        1 => {
            // Future schema >=2 migrations will be added here.
        }
        _ => {
            return Err(newer_schema_error(from_version));
        }
    }
    Ok(())
}

fn newer_schema_error(version: i64) -> SqliteError {
    SqliteError::SqliteFailure(
        rusqlite::ffi::Error {
            code: ErrorCode::DatabaseCorrupt,
            extended_code: 0,
        },
        Some(format!(
            "catalog schema version {version} is newer than supported {TARGET_SCHEMA_VERSION}"
        )),
    )
}
