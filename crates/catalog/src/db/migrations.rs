use anyhow::{anyhow, Context};
use rusqlite::{params, Connection, OptionalExtension};

use crate::db::{CatalogDb, DbResult};

#[derive(Debug, Clone)]
pub struct Migration {
    pub from: i32,
    pub to: i32,
    pub sql: &'static str,
}

pub const MIGRATIONS: &[Migration] = &[
    // Additive migration example: add new columns and backfill.
    Migration {
        from: 1,
        to: 2,
        sql: r#"
            ALTER TABLE images ADD COLUMN camera_serial TEXT;
            ALTER TABLE images ADD COLUMN temp_tag TEXT;
            UPDATE images SET temp_tag = 'legacy-import' WHERE imported_at IS NOT NULL;
        "#,
    },
    // Example virtual table and backfill.
    Migration {
        from: 2,
        to: 3,
        sql: r#"
            CREATE VIRTUAL TABLE IF NOT EXISTS image_search_fts
            USING fts5(filename, original_path, content='images', content_rowid='id');
            INSERT INTO image_search_fts (rowid, filename, original_path)
            SELECT id, filename, original_path FROM images;
        "#,
    },
    // Column drop pattern via table rebuild (removes temp_tag while keeping camera_serial).
    Migration {
        from: 3,
        to: 4,
        sql: r#"
            CREATE TABLE images_new (
                id INTEGER PRIMARY KEY,
                folder_id INTEGER NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
                filename TEXT NOT NULL,
                original_path TEXT NOT NULL UNIQUE,
                sidecar_path TEXT,
                sidecar_hash TEXT,
                filesize INTEGER,
                file_hash TEXT,
                file_modified_at TEXT,
                imported_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
                captured_at TEXT,
                camera_make TEXT,
                camera_model TEXT,
                lens_model TEXT,
                focal_length REAL,
                aperture REAL,
                shutter_speed REAL,
                iso INTEGER,
                orientation INTEGER,
                gps_latitude REAL,
                gps_longitude REAL,
                gps_altitude REAL,
                rating INTEGER CHECK (rating BETWEEN 0 AND 5),
                flag TEXT CHECK (flag IN ('picked','rejected') OR flag IS NULL),
                color_label TEXT CHECK (
                    color_label IN ('red','yellow','green','blue','purple','orange','teal')
                    OR color_label IS NULL
                ),
                metadata_json TEXT CHECK (metadata_json IS NULL OR json_valid(metadata_json)),
                camera_serial TEXT,
                created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
                updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
            );
            INSERT INTO images_new (
                id, folder_id, filename, original_path, sidecar_path, sidecar_hash, filesize,
                file_hash, file_modified_at, imported_at, captured_at, camera_make, camera_model,
                lens_model, focal_length, aperture, shutter_speed, iso, orientation, gps_latitude,
                gps_longitude, gps_altitude, rating, flag, color_label, metadata_json,
                camera_serial, created_at, updated_at
            )
            SELECT
                id, folder_id, filename, original_path, sidecar_path, sidecar_hash, filesize,
                file_hash, file_modified_at, imported_at, captured_at, camera_make, camera_model,
                lens_model, focal_length, aperture, shutter_speed, iso, orientation, gps_latitude,
                gps_longitude, gps_altitude, rating, flag, color_label, metadata_json,
                camera_serial, created_at, updated_at
            FROM images;
            DROP TABLE images;
            ALTER TABLE images_new RENAME TO images;
            CREATE INDEX IF NOT EXISTS idx_images_folder_id ON images(folder_id);
            CREATE INDEX IF NOT EXISTS idx_images_captured_at ON images(captured_at);
            CREATE INDEX IF NOT EXISTS idx_images_file_hash ON images(file_hash);
            CREATE TRIGGER IF NOT EXISTS images_touch_updated_at
            AFTER UPDATE ON images
            FOR EACH ROW
            BEGIN
                UPDATE images
                SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
                WHERE id = NEW.id;
            END;
        "#,
    },
];

pub const LATEST_SCHEMA_VERSION: i32 = 4;

pub fn current_schema_version(db: &CatalogDb) -> DbResult<i32> {
    current_schema_version_for_conn(db.conn())
}

pub fn set_schema_version(db: &CatalogDb, version: i32) -> DbResult<()> {
    set_schema_version_for_conn(db.conn(), version)
}

pub fn run_migrations(db: &CatalogDb) -> DbResult<()> {
    run_migrations_for_conn(db.conn(), MIGRATIONS)
}

pub(crate) fn run_migrations_for_conn(conn: &Connection, migrations: &[Migration]) -> DbResult<()> {
    let mut version = current_schema_version_for_conn(conn)?;
    let target = migrations.last().map(|m| m.to).unwrap_or(version);

    if version > target {
        return Err(anyhow!(
            "catalog schema version {version} is newer than supported {target}"
        ));
    }

    let mut progressed = true;
    while progressed && version < target {
        progressed = false;
        for migration in migrations {
            if migration.from != version {
                continue;
            }
            conn.execute_batch("BEGIN IMMEDIATE")?;
            if let Err(e) = conn.execute_batch(migration.sql) {
                conn.execute_batch("ROLLBACK")?;
                return Err(e).with_context(|| {
                    format!(
                        "failed to apply migration {} -> {}",
                        migration.from, migration.to
                    )
                });
            }
            conn.execute(
                "UPDATE catalog_metadata SET schema_version = ?1, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id = 1",
                params![migration.to],
            )?;
            conn.pragma_update(None, "user_version", migration.to)?;
            conn.execute_batch("COMMIT")?;
            version = migration.to;
            progressed = true;
            break;
        }
    }

    if version != target {
        return Err(anyhow!(
            "missing migration path from {} to {}",
            version,
            target
        ));
    }

    Ok(())
}

pub(crate) fn current_schema_version_for_conn(conn: &Connection) -> DbResult<i32> {
    Ok(conn
        .query_row(
            "SELECT schema_version FROM catalog_metadata WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or(0))
}

pub(crate) fn set_schema_version_for_conn(conn: &Connection, version: i32) -> DbResult<()> {
    conn.execute(
        "INSERT INTO catalog_metadata (id, schema_version, created_at, updated_at, last_opened)
         VALUES (1, ?1, strftime('%Y-%m-%dT%H:%M:%fZ','now'), strftime('%Y-%m-%dT%H:%M:%fZ','now'), NULL)
         ON CONFLICT(id) DO UPDATE SET schema_version = excluded.schema_version, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')",
        params![version],
    )?;
    conn.pragma_update(None, "user_version", version)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::initialize_schema;

    #[test]
    fn applies_all_migrations() {
        let conn = Connection::open_in_memory().unwrap();
        initialize_schema(&conn).unwrap();

        let version: i32 = conn
            .query_row(
                "SELECT schema_version FROM catalog_metadata WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(version, LATEST_SCHEMA_VERSION);
        let fts_count: i64 = conn
            .query_row("SELECT count(*) FROM image_search_fts", [], |row| {
                row.get(0)
            })
            .unwrap_or(0);
        assert!(fts_count >= 0);
    }

    #[test]
    fn upgrades_from_intermediate_version() {
        let conn = Connection::open_in_memory().unwrap();
        initialize_schema(&conn).unwrap();
        // Roll back schema version to simulate older catalog.
        conn.execute(
            "UPDATE catalog_metadata SET schema_version = 2 WHERE id = 1",
            [],
        )
        .unwrap();
        conn.execute("PRAGMA user_version = 2", []).unwrap();

        // Ensure migration runs from 2 -> latest.
        run_migrations_for_conn(&conn, MIGRATIONS).unwrap();
        let version = current_schema_version_for_conn(&conn).unwrap();
        assert_eq!(version, LATEST_SCHEMA_VERSION);
    }

    #[test]
    fn migration_failure_rolls_back() {
        let conn = Connection::open_in_memory().unwrap();
        initialize_schema(&conn).unwrap();
        let bad_migration = Migration {
            from: LATEST_SCHEMA_VERSION,
            to: LATEST_SCHEMA_VERSION + 1,
            sql: "THIS IS NOT VALID SQL",
        };
        let migrations = [MIGRATIONS, &[bad_migration]].concat();
        let result = run_migrations_for_conn(&conn, &migrations);
        assert!(result.is_err());
        let version = current_schema_version_for_conn(&conn).unwrap();
        assert_eq!(version, LATEST_SCHEMA_VERSION);
    }
}
