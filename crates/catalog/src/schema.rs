//! Modern SQLite schema DDL and helper utilities for catalog initialization.

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;

use crate::db::migrations::{self, MIGRATIONS};
use crate::db::{to_rfc3339, to_rfc3339_opt, Folder};

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

    if is_legacy_schema(conn)? {
        migrate_legacy_schema(conn)?;
    }

    let has_metadata_table = table_exists(conn, "catalog_metadata")?;

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

fn table_exists(conn: &Connection, name: &str) -> Result<bool> {
    let exists: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [name],
            |row| row.get(0),
        )
        .optional()?;
    Ok(exists.is_some())
}

fn is_legacy_schema(conn: &Connection) -> Result<bool> {
    if table_exists(conn, "catalog_metadata")? {
        return Ok(false);
    }

    if !table_exists(conn, "images")? {
        return Ok(false);
    }

    let mut has_file_path = false;
    let mut has_folder_id = false;
    let mut stmt = conn.prepare("PRAGMA table_info(images)")?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let name: String = row.get(1)?;
        if name.eq_ignore_ascii_case("file_path") {
            has_file_path = true;
        }
        if name.eq_ignore_ascii_case("folder_id") {
            has_folder_id = true;
        }
    }

    Ok(has_file_path || !has_folder_id)
}

fn migrate_legacy_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch("BEGIN IMMEDIATE")
        .context("failed to begin legacy migration")?;

    let outcome = (|| {
        rename_table_if_exists(conn, "images", "legacy_images")?;
        rename_table_if_exists(conn, "keywords", "legacy_keywords")?;
        rename_table_if_exists(conn, "image_keywords", "legacy_image_keywords")?;
        rename_table_if_exists(conn, "edits", "legacy_edits")?;
        rename_table_if_exists(conn, "settings", "legacy_settings")?;

        conn.execute_batch(CATALOG_SCHEMA_SQL)
            .context("failed to install modern schema during migration")?;

        migrate_legacy_keywords(conn)?;
        migrate_legacy_images(conn)?;
        migrate_legacy_image_keywords(conn)?;
        migrate_legacy_edits(conn)?;
        drop_legacy_tables(conn)?;
        Ok(())
    })();

    match outcome {
        Ok(()) => conn
            .execute_batch("COMMIT")
            .context("failed to commit legacy migration"),
        Err(err) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(err)
        }
    }
}

fn rename_table_if_exists(conn: &Connection, from: &str, to: &str) -> Result<()> {
    if table_exists(conn, from)? {
        conn.execute(&format!("ALTER TABLE {from} RENAME TO {to}"), [])
            .with_context(|| format!("failed to rename table {from} to {to}"))?;
    }
    Ok(())
}

fn migrate_legacy_keywords(conn: &Connection) -> Result<()> {
    if !table_exists(conn, "legacy_keywords")? {
        return Ok(());
    }

    conn.execute(
        "INSERT OR IGNORE INTO keywords (id, keyword) SELECT id, keyword FROM legacy_keywords",
        [],
    )?;

    Ok(())
}

fn migrate_legacy_images(conn: &Connection) -> Result<()> {
    if !table_exists(conn, "legacy_images")? {
        return Ok(());
    }

    let now = Utc::now();
    let mut stmt = conn.prepare(
        "SELECT id, file_path, rating, flags, capture_time_utc, camera_make, camera_model, aperture, shutter, iso, focal_length FROM legacy_images",
    )?;
    let mut rows = stmt.query([])?;

    while let Some(row) = rows.next()? {
        let id: i64 = row.get(0)?;
        let file_path: String = row.get(1)?;
        let rating: Option<i64> = row.get(2)?;
        let capture_raw: Option<String> = row.get(4)?;
        let captured_at: Option<DateTime<Utc>> = capture_raw
            .as_deref()
            .and_then(|raw| DateTime::parse_from_rfc3339(raw).ok())
            .map(|dt| dt.with_timezone(&Utc));
        let camera_make: Option<String> = row.get(5)?;
        let camera_model: Option<String> = row.get(6)?;
        let aperture: Option<f64> = row.get(7)?;
        let shutter_speed: Option<f64> = row.get(8)?;
        let iso: Option<i64> = row.get(9)?;
        let focal_length: Option<f64> = row.get(10)?;

        let folder_path = Path::new(&file_path)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| ".".to_string());
        let folder_id = ensure_folder_id(conn, &folder_path, now)?;

        let filename = Path::new(&file_path)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(&file_path)
            .to_string();

        conn.execute(
            "INSERT INTO images (
                id, folder_id, filename, original_path, sidecar_path, sidecar_hash,
                filesize, file_hash, file_modified_at, imported_at, captured_at, camera_make,
                camera_model, lens_model, focal_length, aperture, shutter_speed, iso, orientation,
                gps_latitude, gps_longitude, gps_altitude, rating, flag, color_label,
                metadata_json, created_at, updated_at
            ) VALUES (
                ?1, ?2, ?3, ?4, NULL, NULL,
                NULL, NULL, NULL, ?5, ?6, ?7,
                ?8, NULL, ?9, ?10, ?11, ?12, NULL,
                NULL, NULL, NULL, ?13, NULL, NULL,
                NULL, ?14, ?14
            )",
            params![
                id,
                folder_id,
                filename,
                file_path,
                to_rfc3339(now),
                to_rfc3339_opt(captured_at),
                camera_make,
                camera_model,
                focal_length,
                aperture,
                shutter_speed,
                iso,
                rating,
                to_rfc3339(now)
            ],
        )?;
    }

    Ok(())
}

fn ensure_folder_id(conn: &Connection, path: &str, now: DateTime<Utc>) -> Result<i64> {
    if let Some(existing) = Folder::find_by_path(conn, path)? {
        return Ok(existing.id);
    }

    let folder = Folder {
        id: 0,
        path: path.to_string(),
        created_at: now,
        updated_at: now,
    };

    folder.insert(conn)
}

fn migrate_legacy_image_keywords(conn: &Connection) -> Result<()> {
    if !table_exists(conn, "legacy_image_keywords")? {
        return Ok(());
    }

    conn.execute(
        "INSERT OR IGNORE INTO image_keywords (image_id, keyword_id, assigned_at)
         SELECT image_id, keyword_id, strftime('%Y-%m-%dT%H:%M:%fZ','now') FROM legacy_image_keywords",
        [],
    )?;

    Ok(())
}

fn migrate_legacy_edits(conn: &Connection) -> Result<()> {
    if !table_exists(conn, "legacy_edits")? {
        return Ok(());
    }

    conn.execute(
        "INSERT INTO edits (
            image_id, exposure, contrast, highlights, shadows, temperature, tint, updated_at
        )
        SELECT image_id, exposure, contrast, highlights, shadows, temperature, tint, updated_at
        FROM legacy_edits",
        [],
    )?;

    Ok(())
}

fn drop_legacy_tables(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "DROP TABLE IF EXISTS legacy_images;
         DROP TABLE IF EXISTS legacy_keywords;
         DROP TABLE IF EXISTS legacy_image_keywords;
         DROP TABLE IF EXISTS legacy_edits;
         DROP TABLE IF EXISTS legacy_settings;",
    )?;
    Ok(())
}
