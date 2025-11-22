pub mod db;
pub mod schema;

use app_settings::AppSettings;
use chrono::{DateTime, Utc};
use core_types::ImageFlags;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CatalogError {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Time parse error: {0}")]
    Time(#[from] chrono::ParseError),

    #[error("Settings error: {0}")]
    Settings(#[from] app_settings::AppSettingsError),

    #[error("Unsupported catalog version: {0}")]
    UnsupportedVersion(i64),
}

pub type Result<T> = std::result::Result<T, CatalogError>;

#[derive(Debug, Clone)]
pub struct CatalogPath(PathBuf);

impl CatalogPath {
    pub fn new<P: AsRef<Path>>(path: P) -> Self {
        let mut normalized = path.as_ref().to_path_buf();
        if normalized
            .extension()
            .and_then(|s| s.to_str())
            .filter(|ext| {
                ext.eq_ignore_ascii_case("zenithphotocatalog") || ext.eq_ignore_ascii_case("sqlite")
            })
            .is_none()
        {
            normalized.set_extension("zenithphotocatalog");
        }
        Self(normalized)
    }

    pub fn as_path(&self) -> &Path {
        &self.0
    }

    pub fn into_path(self) -> PathBuf {
        self.0
    }
}

#[derive(Debug)]
pub struct Catalog {
    conn: Connection,
    path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageMetadata {
    pub id: i64,
    pub file_path: PathBuf,
    pub rating: Option<i32>,
    pub flags: Option<ImageFlags>,
    pub capture_time_utc: Option<DateTime<Utc>>,
    pub camera_make: Option<String>,
    pub camera_model: Option<String>,
    pub aperture: Option<f64>,
    pub shutter: Option<f64>,
    pub iso: Option<i32>,
    pub focal_length: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewImage {
    pub file_path: PathBuf,
    pub rating: Option<i32>,
    pub flags: Option<ImageFlags>,
    pub capture_time_utc: Option<DateTime<Utc>>,
    pub camera_make: Option<String>,
    pub camera_model: Option<String>,
    pub aperture: Option<f64>,
    pub shutter: Option<f64>,
    pub iso: Option<i32>,
    pub focal_length: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageEdits {
    pub exposure: f64,
    pub contrast: f64,
    pub highlights: f64,
    pub shadows: f64,
    pub white_balance: f64,
    pub temperature: f64,
    pub tint: f64,
    pub updated_at: DateTime<Utc>,
}

impl Default for ImageEdits {
    fn default() -> Self {
        Self {
            exposure: 0.0,
            contrast: 0.0,
            highlights: 0.0,
            shadows: 0.0,
            white_balance: 0.0,
            temperature: 6500.0,
            tint: 0.0,
            updated_at: Utc::now(),
        }
    }
}

impl Catalog {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = CatalogPath::new(path).into_path();
        let conn = Connection::open(&path)?;
        configure_connection(&conn)?;
        let mut catalog = Catalog { conn, path };
        catalog.migrate()?;
        Ok(catalog)
    }

    pub fn create(path: impl AsRef<Path>) -> Result<Self> {
        let catalog_path = CatalogPath::new(path).into_path();
        if let Some(parent) = catalog_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(&catalog_path)?;
        configure_connection(&conn)?;
        let mut catalog = Catalog {
            conn,
            path: catalog_path,
        };
        catalog.create_schema()?;
        Ok(catalog)
    }

    pub fn open_or_create(path: impl AsRef<Path>) -> Result<Self> {
        let catalog_path = CatalogPath::new(path).into_path();
        if catalog_path.exists() {
            Self::open(catalog_path)
        } else {
            Self::create(catalog_path)
        }
    }

    pub fn last_used() -> Option<PathBuf> {
        AppSettings::load().ok().and_then(|s| s.last_catalog)
    }

    pub fn set_last_used(path: impl AsRef<Path>) -> Result<()> {
        let mut settings = AppSettings::load().unwrap_or_default();
        let normalized = CatalogPath::new(path).into_path();
        settings.set_last_catalog(normalized);
        settings.save()?;
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn insert_image(&self, image: NewImage) -> Result<i64> {
        self.conn.execute(
            r#"INSERT INTO images (
                file_path, rating, flags, capture_time_utc, camera_make, camera_model,
                aperture, shutter, iso, focal_length
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)"#,
            params![
                image.file_path.to_string_lossy(),
                image.rating,
                image.flags.map(|f| f.bits() as i64),
                image
                    .capture_time_utc
                    .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
                image.camera_make,
                image.camera_model,
                image.aperture,
                image.shutter,
                image.iso,
                image.focal_length
            ],
        )?;

        Ok(self.conn.last_insert_rowid())
    }

    pub fn update_image_metadata(&self, id: i64, updates: ImageMetadataUpdate) -> Result<()> {
        self.conn.execute(
            r#"UPDATE images SET
                rating = ?1,
                flags = ?2,
                capture_time_utc = ?3,
                camera_make = ?4,
                camera_model = ?5,
                aperture = ?6,
                shutter = ?7,
                iso = ?8,
                focal_length = ?9
              WHERE id = ?10"#,
            params![
                updates.rating,
                updates.flags.map(|f| f.bits() as i64),
                updates
                    .capture_time_utc
                    .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
                updates.camera_make,
                updates.camera_model,
                updates.aperture,
                updates.shutter,
                updates.iso,
                updates.focal_length,
                id
            ],
        )?;

        Ok(())
    }

    pub fn get_image_by_path(&self, path: impl AsRef<Path>) -> Result<Option<ImageMetadata>> {
        let path_str = path.as_ref().to_string_lossy().to_string();
        self.conn
            .query_row(
                r#"SELECT id, file_path, rating, flags, capture_time_utc, camera_make,
                          camera_model, aperture, shutter, iso, focal_length
                   FROM images WHERE file_path = ?1"#,
                params![path_str],
                row_to_image,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn list_images(&self) -> Result<Vec<ImageMetadata>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT id, file_path, rating, flags, capture_time_utc, camera_make,
                      camera_model, aperture, shutter, iso, focal_length
               FROM images
               ORDER BY id ASC"#,
        )?;
        let iter = stmt.query_map([], row_to_image)?;

        let mut images = Vec::new();
        for entry in iter {
            images.push(entry?);
        }
        Ok(images)
    }

    pub fn delete_image(&mut self, id: i64) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM image_keywords WHERE image_id = ?1",
            params![id],
        )?;
        tx.execute("DELETE FROM edits WHERE image_id = ?1", params![id])?;
        tx.execute("DELETE FROM images WHERE id = ?1", params![id])?;
        tx.commit()?;
        Ok(())
    }

    pub fn add_keyword(&self, keyword: &str) -> Result<i64> {
        self.conn.execute(
            "INSERT OR IGNORE INTO keywords (keyword) VALUES (?1)",
            params![keyword],
        )?;
        self.conn
            .query_row(
                "SELECT id FROM keywords WHERE keyword = ?1",
                params![keyword],
                |row| row.get(0),
            )
            .map_err(Into::into)
    }

    pub fn add_keyword_to_image(&mut self, image_id: i64, keyword: &str) -> Result<()> {
        let tx = self.conn.transaction()?;
        let keyword_id: i64 = {
            tx.execute(
                "INSERT OR IGNORE INTO keywords (keyword) VALUES (?1)",
                params![keyword],
            )?;
            tx.query_row(
                "SELECT id FROM keywords WHERE keyword = ?1",
                params![keyword],
                |row| row.get(0),
            )?
        };

        tx.execute(
            "INSERT OR IGNORE INTO image_keywords (image_id, keyword_id) VALUES (?1, ?2)",
            params![image_id, keyword_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn keywords_for_image(&self, image_id: i64) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT k.keyword
               FROM keywords k
               INNER JOIN image_keywords ik ON k.id = ik.keyword_id
               WHERE ik.image_id = ?1
               ORDER BY k.keyword"#,
        )?;
        let rows = stmt.query_map(params![image_id], |row| row.get(0))?;
        let mut items = Vec::new();
        for row in rows {
            items.push(row?);
        }
        Ok(items)
    }

    pub fn upsert_edits(&self, image_id: i64, edits: ImageEdits) -> Result<()> {
        self.conn.execute(
            r#"INSERT INTO edits (
                image_id, exposure, contrast, highlights, shadows,
                white_balance, temperature, tint, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            ON CONFLICT(image_id) DO UPDATE SET
                exposure = excluded.exposure,
                contrast = excluded.contrast,
                highlights = excluded.highlights,
                shadows = excluded.shadows,
                white_balance = excluded.white_balance,
                temperature = excluded.temperature,
                tint = excluded.tint,
                updated_at = excluded.updated_at"#,
            params![
                image_id,
                edits.exposure,
                edits.contrast,
                edits.highlights,
                edits.shadows,
                edits.white_balance,
                edits.temperature,
                edits.tint,
                edits
                    .updated_at
                    .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
            ],
        )?;
        Ok(())
    }

    pub fn load_edits(&self, image_id: i64) -> Result<Option<ImageEdits>> {
        self.conn
            .query_row(
                r#"SELECT exposure, contrast, highlights, shadows,
                          white_balance, temperature, tint, updated_at
                   FROM edits WHERE image_id = ?1"#,
                params![image_id],
                |row| {
                    let updated: String = row.get(7)?;
                    let parsed_updated = DateTime::parse_from_rfc3339(&updated).map_err(|e| {
                        rusqlite::Error::FromSqlConversionFailure(
                            7,
                            rusqlite::types::Type::Text,
                            Box::new(e),
                        )
                    })?;
                    Ok(ImageEdits {
                        exposure: row.get(0)?,
                        contrast: row.get(1)?,
                        highlights: row.get(2)?,
                        shadows: row.get(3)?,
                        white_balance: row.get(4)?,
                        temperature: row.get(5)?,
                        tint: row.get(6)?,
                        updated_at: parsed_updated.with_timezone(&Utc),
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn vacuum(&self) -> Result<()> {
        self.conn.execute_batch("VACUUM")?;
        Ok(())
    }

    pub fn maintenance(&self) -> Result<()> {
        self.conn
            .execute_batch("PRAGMA optimize; ANALYZE; VACUUM;")?;
        Ok(())
    }

    fn migrate(&mut self) -> Result<()> {
        let version: i64 = self
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))?;
        match version {
            0 => self.create_schema()?,
            1 => {}
            v => return Err(CatalogError::UnsupportedVersion(v)),
        }
        Ok(())
    }

    fn create_schema(&mut self) -> Result<()> {
        self.conn.execute_batch(SCHEMA_V1)?;
        self.conn.execute("PRAGMA user_version = 1", [])?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageMetadataUpdate {
    pub rating: Option<i32>,
    pub flags: Option<ImageFlags>,
    pub capture_time_utc: Option<DateTime<Utc>>,
    pub camera_make: Option<String>,
    pub camera_model: Option<String>,
    pub aperture: Option<f64>,
    pub shutter: Option<f64>,
    pub iso: Option<i32>,
    pub focal_length: Option<f64>,
}

fn row_to_image(row: &rusqlite::Row<'_>) -> rusqlite::Result<ImageMetadata> {
    let capture_raw: Option<String> = row.get(4)?;
    let capture_time = match capture_raw {
        Some(raw) => {
            let parsed = DateTime::parse_from_rfc3339(&raw).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    4,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?;
            Some(parsed.with_timezone(&Utc))
        }
        None => None,
    };

    let flags: Option<i64> = row.get(3)?;
    let flags = flags.map(|bits| ImageFlags::from_bits_truncate(bits as u8));

    Ok(ImageMetadata {
        id: row.get(0)?,
        file_path: PathBuf::from(row.get::<_, String>(1)?),
        rating: row.get(2)?,
        flags,
        capture_time_utc: capture_time,
        camera_make: row.get(5)?,
        camera_model: row.get(6)?,
        aperture: row.get(7)?,
        shutter: row.get(8)?,
        iso: row.get(9)?,
        focal_length: row.get(10)?,
    })
}

fn configure_connection(conn: &Connection) -> Result<()> {
    conn.execute_batch("PRAGMA foreign_keys = ON;")?;
    Ok(())
}

const SCHEMA_V1: &str = r#"
CREATE TABLE IF NOT EXISTS images(
    id INTEGER PRIMARY KEY,
    file_path TEXT UNIQUE NOT NULL,
    rating INTEGER,
    flags INTEGER,
    capture_time_utc TEXT,
    camera_make TEXT,
    camera_model TEXT,
    aperture REAL,
    shutter REAL,
    iso INTEGER,
    focal_length REAL
);

CREATE TABLE IF NOT EXISTS keywords(
    id INTEGER PRIMARY KEY,
    keyword TEXT UNIQUE NOT NULL
);

CREATE TABLE IF NOT EXISTS image_keywords(
    image_id INTEGER,
    keyword_id INTEGER,
    PRIMARY KEY(image_id, keyword_id)
);

CREATE TABLE IF NOT EXISTS edits(
    image_id INTEGER PRIMARY KEY,
    exposure REAL,
    contrast REAL,
    highlights REAL,
    shadows REAL,
    white_balance REAL,
    temperature REAL,
    tint REAL,
    updated_at TEXT
);

CREATE TABLE IF NOT EXISTS settings(
    key TEXT PRIMARY KEY,
    value TEXT
);

PRAGMA foreign_keys = ON;
"#;
