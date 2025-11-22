use crate::db::{query_all, CatalogDb, DbResult, Folder, Image, Keyword};
use anyhow::Context;
use rusqlite::params;

/// Perform keyword full-text search.
pub fn search_keywords(db: &CatalogDb, query: &str) -> DbResult<Vec<Keyword>> {
    query_all(
        db,
        "SELECT k.id, k.keyword
         FROM fts_keywords f
         JOIN keywords k ON k.id = f.rowid
         WHERE fts_keywords MATCH ?1
         ORDER BY bm25(fts_keywords)",
        params![query],
        Keyword::from_row,
    )
}

/// Perform image filename/path/metadata search.
pub fn search_images(db: &CatalogDb, query: &str) -> DbResult<Vec<Image>> {
    query_all(
        db,
        "SELECT
            i.id, i.folder_id, i.filename, i.original_path, i.sidecar_path, i.sidecar_hash,
            i.filesize, i.file_hash, i.file_modified_at, i.imported_at, i.captured_at,
            i.camera_make, i.camera_model, i.lens_model, i.focal_length, i.aperture,
            i.shutter_speed, i.iso, i.orientation, i.gps_latitude, i.gps_longitude,
            i.gps_altitude, i.rating, i.flag, i.color_label, i.metadata_json,
            i.created_at, i.updated_at
         FROM fts_images f
         JOIN images i ON i.id = f.rowid
         WHERE fts_images MATCH ?1
         ORDER BY bm25(fts_images)",
        params![query],
        Image::from_row,
    )
}

/// Perform folder path search.
pub fn search_folders(db: &CatalogDb, query: &str) -> DbResult<Vec<Folder>> {
    query_all(
        db,
        "SELECT f2.id, f2.path, f2.created_at, f2.updated_at
         FROM fts_folders f
         JOIN folders f2 ON f2.id = f.rowid
         WHERE fts_folders MATCH ?1
         ORDER BY bm25(fts_folders)",
        params![query],
        Folder::from_row,
    )
}

/// Rebuild every FTS table from the base tables.
pub fn rebuild_fts(db: &CatalogDb) -> DbResult<()> {
    db.conn().execute_batch("BEGIN IMMEDIATE")?;
    db.conn().execute_batch(
        "
        DROP TABLE IF EXISTS fts_keywords;
        DROP TABLE IF EXISTS fts_images;
        DROP TABLE IF EXISTS fts_folders;
        CREATE VIRTUAL TABLE IF NOT EXISTS fts_keywords
            USING fts5(keyword, content='', tokenize='unicode61');
        CREATE VIRTUAL TABLE IF NOT EXISTS fts_images
            USING fts5(
                filename,
                original_path,
                metadata_json,
                content='',
                tokenize='unicode61'
            );
        CREATE VIRTUAL TABLE IF NOT EXISTS fts_folders
            USING fts5(path, content='', tokenize='unicode61');
        ",
    )?;
    db.conn().execute(
        "INSERT INTO fts_keywords(rowid, keyword) SELECT id, keyword FROM keywords",
        [],
    )?;
    db.conn().execute(
        "INSERT INTO fts_images(rowid, filename, original_path, metadata_json)
         SELECT id, filename, original_path, COALESCE(metadata_json, '') FROM images",
        [],
    )?;
    db.conn().execute(
        "INSERT INTO fts_folders(rowid, path) SELECT id, path FROM folders",
        [],
    )?;
    db.conn()
        .execute_batch("COMMIT")
        .context("failed to rebuild FTS indexes")?;
    Ok(())
}

/// Example integration (Slint UI pseudo-code):
/// ```ignore
/// ui.on_search_query(move |text| {
///     let results = search_images(&db, &text).unwrap_or_default();
///     // Bind `results` to a list model for display.
/// });
/// ```

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Folder, Image, Keyword};
    use crate::schema::initialize_schema;
    use chrono::Utc;

    fn seed_basic(db: &CatalogDb) -> (i64, i64, i64) {
        let folder = Folder {
            id: 0,
            path: "/photos/ftstest".into(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let folder_id = folder.insert(db).unwrap();

        let keyword = Keyword {
            id: 0,
            keyword: "sunset beach".into(),
        };
        let keyword_id = keyword.insert(db).unwrap();

        let image = Image {
            id: 0,
            folder_id,
            filename: "sunset_001.dng".into(),
            original_path: "/photos/ftstest/sunset_001.dng".into(),
            sidecar_path: None,
            sidecar_hash: None,
            filesize: Some(123),
            file_hash: None,
            file_modified_at: None,
            imported_at: Utc::now(),
            captured_at: None,
            camera_make: None,
            camera_model: None,
            lens_model: None,
            focal_length: None,
            aperture: None,
            shutter_speed: None,
            iso: None,
            orientation: None,
            gps_latitude: None,
            gps_longitude: None,
            gps_altitude: None,
            rating: None,
            flag: None,
            color_label: None,
            metadata_json: Some(serde_json::json!({"location": "Beach", "mood": "Sunset"})),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let image_id = image.insert(db).unwrap();

        let ik = crate::db::ImageKeyword {
            image_id,
            keyword_id,
            assigned_at: Utc::now(),
        };
        ik.insert(db).unwrap();

        (folder_id, image_id, keyword_id)
    }

    #[test]
    fn search_returns_results() {
        let db = CatalogDb::in_memory().unwrap();
        initialize_schema(db.conn()).unwrap();
        let (_folder_id, image_id, _keyword_id) = seed_basic(&db);

        let imgs = search_images(&db, "sunset").unwrap();
        assert_eq!(imgs.len(), 1);
        assert_eq!(imgs[0].id, image_id);

        let folders = search_folders(&db, "photos").unwrap();
        assert_eq!(folders.len(), 1);

        let keywords = search_keywords(&db, "sunset").unwrap();
        assert_eq!(keywords.len(), 1);
    }

    #[test]
    fn rebuild_repopulates_indexes() {
        let db = CatalogDb::in_memory().unwrap();
        initialize_schema(db.conn()).unwrap();
        seed_basic(&db);

        rebuild_fts(&db).unwrap();
        let imgs = search_images(&db, "sunset").unwrap();
        assert_eq!(imgs.len(), 1);
    }
}
