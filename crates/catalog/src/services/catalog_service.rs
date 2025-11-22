use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use blake3::Hasher;
use chrono::{DateTime, Utc};
use image::{DynamicImage, ImageOutputFormat};
use rusqlite::params;
use serde_json::Value;

use crate::db::search;
use crate::db::{
    query_all, query_one, query_optional, to_json, to_rfc3339, to_rfc3339_opt, CatalogDb,
    Collection, DbHandle, Folder, Image, ImageKeyword, Keyword, Preview, Thumbnail,
};

/// Alias the low-level edit record for service consumers.
pub type Edits = crate::db::Edit;

/// Aggregated metadata and keywords for a single image.
#[derive(Debug, Clone)]
pub struct ImageDetails {
    pub image: Image,
    pub keywords: Vec<String>,
}

/// High-level catalog operations that sit above the raw ORM bindings.
pub struct CatalogService {
    pub db: CatalogDb,
}

impl CatalogService {
    pub fn new(db: CatalogDb) -> Self {
        Self { db }
    }

    pub fn list_folders(&self) -> Result<Vec<Folder>> {
        Folder::load_all(&self.db).context("failed to list folders")
    }

    pub fn list_images_in_folder(&self, folder_path: &Path) -> Result<Vec<Image>> {
        let normalized = folder_path.to_string_lossy().to_string();
        let Some(folder) = Folder::find_by_path(&self.db, &normalized)? else {
            return Ok(Vec::new());
        };
        Image::find_by_folder(&self.db, folder.id)
            .with_context(|| format!("failed to list images for folder {}", folder_path.display()))
    }

    pub fn load_thumbnail(&self, image_id: i64) -> Result<Option<Thumbnail>> {
        query_optional(
            &self.db,
            "SELECT image_id, thumb_256, thumb_1024, updated_at FROM thumbnails WHERE image_id = ?1",
            params![image_id],
            Thumbnail::from_row,
        )
        .context("failed to load thumbnail")
    }

    pub fn load_metadata(&self, image_id: i64) -> Result<ImageDetails> {
        let image = Image::load(&self.db, image_id)
            .with_context(|| format!("failed to load image id={image_id}"))?;
        let keywords = ImageKeyword::list_keywords_for_image(&self.db, image_id)
            .context("failed to load keywords for image")?
            .into_iter()
            .map(|k| k.keyword)
            .collect();

        Ok(ImageDetails { image, keywords })
    }

    pub fn update_keywords(&self, image_id: i64, keywords: &[String]) -> Result<()> {
        let desired: HashSet<String> = keywords
            .iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        let existing_keywords = ImageKeyword::list_keywords_for_image(&self.db, image_id)?;
        let existing: HashSet<String> = existing_keywords
            .iter()
            .map(|k| k.keyword.clone())
            .collect();

        for kw in desired.iter().filter(|kw| !existing.contains(*kw)) {
            let keyword = Keyword::get_or_create(&self.db, kw)
                .with_context(|| format!("failed to upsert keyword {kw}"))?;
            ImageKeyword {
                image_id,
                keyword_id: keyword.id,
                assigned_at: Utc::now(),
            }
            .insert(&self.db)?;
        }

        for kw in existing_keywords {
            if !desired.contains(&kw.keyword) {
                ImageKeyword::delete(&self.db, image_id, kw.id)?;
            }
        }

        Ok(())
    }

    pub fn update_rating(&self, image_id: i64, rating: i32) -> Result<()> {
        self.db
            .execute(
                "UPDATE images SET rating = ?1, updated_at = ?2 WHERE id = ?3",
                params![rating, to_rfc3339(Utc::now()), image_id],
            )
            .with_context(|| format!("failed to update rating for image_id={image_id}"))?;
        Ok(())
    }

    pub fn update_flag(&self, image_id: i64, flag: Option<String>) -> Result<()> {
        self.db
            .execute(
                "UPDATE images SET flag = ?1, updated_at = ?2 WHERE id = ?3",
                params![flag, to_rfc3339(Utc::now()), image_id],
            )
            .with_context(|| format!("failed to update flag for image_id={image_id}"))?;
        Ok(())
    }

    pub fn update_color_label(&self, image_id: i64, label: Option<String>) -> Result<()> {
        let normalized = label.map(|s| s.to_ascii_lowercase());
        self.db
            .execute(
                "UPDATE images SET color_label = ?1, updated_at = ?2 WHERE id = ?3",
                params![normalized, to_rfc3339(Utc::now()), image_id],
            )
            .with_context(|| format!("failed to update color label for image_id={image_id}"))?;
        Ok(())
    }

    pub fn import_image(&self, path: &Path) -> Result<Image> {
        let metadata = fs::metadata(path)
            .with_context(|| format!("failed to read file metadata for {:?}", path))?;
        let folder_path = Self::parent_path(path);
        let folder = self.ensure_folder(&folder_path)?;
        let file_hash = Self::compute_file_hash(path)
            .with_context(|| format!("failed to hash file {:?}", path))?;

        let metadata_json = match self.extract_exif_metadata(path)? {
            Some(json) => Some(json),
            None => self.scan_raw_metadata(path)?,
        };

        let filename = path
            .file_name()
            .and_then(|s| s.to_str())
            .context("image path is missing a valid filename")?
            .to_string();

        let now = Utc::now();
        let image = Image {
            id: 0,
            folder_id: folder.id,
            filename,
            original_path: path.to_string_lossy().to_string(),
            sidecar_path: None,
            sidecar_hash: None,
            filesize: Some(metadata.len() as i64),
            file_hash: Some(file_hash),
            file_modified_at: Self::modified_time(&metadata),
            imported_at: now,
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
            metadata_json,
            created_at: now,
            updated_at: now,
        };

        let id = image.insert(&self.db)?;
        let mut saved = image;
        saved.id = id;
        Ok(saved)
    }

    pub fn apply_edits(&self, image_id: i64, edits: Edits) -> Result<()> {
        let updated_at = edits.updated_at.unwrap_or_else(Utc::now);
        let parametric_curve_json = edits
            .parametric_curve_json
            .as_ref()
            .map(to_json)
            .transpose()?;
        let color_grading_json = edits.color_grading_json.as_ref().map(to_json).transpose()?;
        let crop_json = edits.crop_json.as_ref().map(to_json).transpose()?;
        let masking_json = edits.masking_json.as_ref().map(to_json).transpose()?;

        self.db
            .execute(
                "INSERT INTO edits (
                    image_id, exposure, contrast, highlights, shadows, whites, blacks,
                    vibrance, saturation, temperature, tint, texture, clarity, dehaze,
                    parametric_curve_json, color_grading_json, crop_json, masking_json, updated_at
                ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7,
                    ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                    ?15, ?16, ?17, ?18, ?19
                )
                ON CONFLICT(image_id) DO UPDATE SET
                    exposure = excluded.exposure,
                    contrast = excluded.contrast,
                    highlights = excluded.highlights,
                    shadows = excluded.shadows,
                    whites = excluded.whites,
                    blacks = excluded.blacks,
                    vibrance = excluded.vibrance,
                    saturation = excluded.saturation,
                    temperature = excluded.temperature,
                    tint = excluded.tint,
                    texture = excluded.texture,
                    clarity = excluded.clarity,
                    dehaze = excluded.dehaze,
                    parametric_curve_json = excluded.parametric_curve_json,
                    color_grading_json = excluded.color_grading_json,
                    crop_json = excluded.crop_json,
                    masking_json = excluded.masking_json,
                    updated_at = excluded.updated_at",
                params![
                    image_id,
                    edits.exposure,
                    edits.contrast,
                    edits.highlights,
                    edits.shadows,
                    edits.whites,
                    edits.blacks,
                    edits.vibrance,
                    edits.saturation,
                    edits.temperature,
                    edits.tint,
                    edits.texture,
                    edits.clarity,
                    edits.dehaze,
                    parametric_curve_json,
                    color_grading_json,
                    crop_json,
                    masking_json,
                    to_rfc3339_opt(Some(updated_at))
                ],
            )
            .with_context(|| format!("failed to upsert edits for image_id={image_id}"))?;
        Ok(())
    }

    pub fn add_keyword_to_image(&self, image_id: i64, keyword: &str) -> Result<()> {
        let keyword = keyword.trim();
        if keyword.is_empty() {
            return Ok(());
        }

        let keyword = Keyword::get_or_create(&self.db, keyword)
            .with_context(|| format!("failed to resolve keyword '{keyword}'"))?;
        self.db
            .execute(
                "INSERT OR IGNORE INTO image_keywords (image_id, keyword_id, assigned_at)
                 VALUES (?1, ?2, ?3)",
                params![image_id, keyword.id, to_rfc3339(Utc::now())],
            )
            .with_context(|| {
                format!(
                    "failed to associate keyword {} with image {}",
                    keyword.keyword, image_id
                )
            })?;
        Ok(())
    }

    pub fn remove_keyword_from_image(&self, image_id: i64, keyword: &str) -> Result<()> {
        let existing = query_optional(
            &self.db,
            "SELECT id, keyword FROM keywords WHERE keyword = ?1",
            params![keyword],
            Keyword::from_row,
        )?;

        if let Some(keyword) = existing {
            ImageKeyword::delete(&self.db, image_id, keyword.id).with_context(|| {
                format!(
                    "failed to remove keyword {} from image {}",
                    keyword.keyword, image_id
                )
            })?;
        }

        Ok(())
    }

    pub fn list_images_in_collection(&self, collection_id: i64) -> Result<Vec<Image>> {
        Collection::list_images(&self.db, collection_id)
            .with_context(|| format!("failed to list images for collection {collection_id}"))
    }

    pub fn create_collection(&self, name: &str) -> Result<Collection> {
        let now = Utc::now();
        let collection = Collection {
            id: 0,
            name: name.to_string(),
            parent_id: None,
            created_at: now,
            updated_at: now,
        };

        let id = collection.insert(&self.db)?;
        Ok(Collection { id, ..collection })
    }

    pub fn add_image_to_collection(&self, collection_id: i64, image_id: i64) -> Result<()> {
        Collection::add_image(&self.db, collection_id, image_id).with_context(|| {
            format!(
                "failed to add image {} to collection {}",
                image_id, collection_id
            )
        })
    }

    pub fn search(&self, query: &str) -> Result<Vec<Image>> {
        search::search_images(&self.db, query).context("failed to run image search")
    }

    pub fn count_images(&self) -> Result<i64> {
        query_one(&self.db, "SELECT COUNT(*) FROM images", [], |row| {
            Ok(row.get::<_, i64>(0)?)
        })
        .context("failed to count images")
    }

    pub fn count_by_camera(&self) -> Result<HashMap<String, usize>> {
        let rows: Vec<(String, usize)> = query_all(
            &self.db,
            "SELECT COALESCE(camera_model, camera_make, 'Unknown') AS camera, COUNT(*)
             FROM images
             GROUP BY camera",
            [],
            |row| Ok((row.get(0)?, row.get::<_, i64>(1)? as usize)),
        )?;

        let mut out = HashMap::new();
        for (camera, count) in rows {
            out.insert(camera, count);
        }
        Ok(out)
    }

    pub fn recently_imported(&self, limit: usize) -> Result<Vec<Image>> {
        query_all(
            &self.db,
            "SELECT
                id, folder_id, filename, original_path, sidecar_path, sidecar_hash, filesize,
                file_hash, file_modified_at, imported_at, captured_at, camera_make,
                camera_model, lens_model, focal_length, aperture, shutter_speed, iso,
                orientation, gps_latitude, gps_longitude, gps_altitude, rating, flag,
                color_label, metadata_json, created_at, updated_at
             FROM images
             ORDER BY imported_at DESC
             LIMIT ?1",
            params![limit as i64],
            Image::from_row,
        )
        .context("failed to list recently imported images")
    }

    pub fn images_with_rating(&self, rating: i32) -> Result<Vec<Image>> {
        query_all(
            &self.db,
            "SELECT
                id, folder_id, filename, original_path, sidecar_path, sidecar_hash, filesize,
                file_hash, file_modified_at, imported_at, captured_at, camera_make,
                camera_model, lens_model, focal_length, aperture, shutter_speed, iso,
                orientation, gps_latitude, gps_longitude, gps_altitude, rating, flag,
                color_label, metadata_json, created_at, updated_at
             FROM images
             WHERE rating = ?1",
            params![rating],
            Image::from_row,
        )
        .with_context(|| format!("failed to load images with rating {rating}"))
    }

    pub fn upsert_thumbnail(
        &self,
        image_id: i64,
        thumb_256: Option<Vec<u8>>,
        thumb_1024: Option<Vec<u8>>,
    ) -> Result<Thumbnail> {
        let updated_at = Utc::now();
        self.db
            .execute(
                "INSERT INTO thumbnails (image_id, thumb_256, thumb_1024, updated_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(image_id) DO UPDATE SET
                    thumb_256 = excluded.thumb_256,
                    thumb_1024 = excluded.thumb_1024,
                    updated_at = excluded.updated_at",
                params![
                    image_id,
                    thumb_256.as_ref(),
                    thumb_1024.as_ref(),
                    to_rfc3339(updated_at)
                ],
            )
            .with_context(|| format!("failed to upsert thumbnail for image_id={image_id}"))?;

        Ok(Thumbnail {
            image_id,
            thumb_256,
            thumb_1024,
            updated_at,
        })
    }

    pub fn upsert_preview_placeholder(
        &self,
        image_id: i64,
        preview_blob: Option<Vec<u8>>,
    ) -> Result<Preview> {
        let updated_at = Utc::now();
        self.db
            .execute(
                "INSERT INTO previews (image_id, preview_blob, updated_at)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(image_id) DO UPDATE SET
                    preview_blob = excluded.preview_blob,
                    updated_at = excluded.updated_at",
                params![image_id, preview_blob.as_ref(), to_rfc3339(updated_at)],
            )
            .with_context(|| format!("failed to upsert preview for image_id={image_id}"))?;

        Ok(Preview {
            image_id,
            preview_blob,
            updated_at,
        })
    }

    /// Generate and persist thumbnails for an image. Returns `None` when decoding fails,
    /// allowing callers to continue importing while recording the original path.
    pub fn generate_thumbnail(&self, image_id: i64, path: &Path) -> Result<Option<Thumbnail>> {
        match Self::load_image_for_thumbnail(path) {
            Ok(img) => {
                let thumb_256 =
                    Self::thumbnail_bytes(&img, 256).context("failed to encode 256px thumbnail")?;
                let thumb_1024 = Self::thumbnail_bytes(&img, 1024)
                    .context("failed to encode 1024px thumbnail")?;

                let thumb = self.upsert_thumbnail(image_id, Some(thumb_256), Some(thumb_1024))?;
                Ok(Some(thumb))
            }
            Err(err) => {
                eprintln!("Thumbnail decode failed for {:?}: {err}", path);
                Ok(None)
            }
        }
    }

    /// Placeholder for future RAW/sidecar parsing.
    pub fn scan_raw_metadata(&self, _path: &Path) -> Result<Option<Value>> {
        // TODO: Plug in RAW parsers (cr2/nef/raf/arw) and surface metadata here.
        Ok(None)
    }

    pub fn find_image_by_original_path(&self, path: &Path) -> Result<Option<Image>> {
        query_optional(
            &self.db,
            "SELECT
                id, folder_id, filename, original_path, sidecar_path, sidecar_hash, filesize,
                file_hash, file_modified_at, imported_at, captured_at, camera_make,
                camera_model, lens_model, focal_length, aperture, shutter_speed, iso,
                orientation, gps_latitude, gps_longitude, gps_altitude, rating, flag,
                color_label, metadata_json, created_at, updated_at
             FROM images
             WHERE original_path = ?1",
            params![path.to_string_lossy()],
            Image::from_row,
        )
        .with_context(|| format!("failed to check for existing image at {}", path.display()))
    }

    pub fn find_image_by_hash(&self, hash: &str) -> Result<Option<Image>> {
        Image::find_by_hash(&self.db, hash)
            .with_context(|| format!("failed to check for existing image hash={hash}"))
    }

    fn ensure_folder(&self, path: &Path) -> Result<Folder> {
        let path_str = path.to_string_lossy().to_string();
        if let Some(existing) = Folder::find_by_path(&self.db, &path_str)? {
            return Ok(existing);
        }

        let now = Utc::now();
        let folder = Folder {
            id: 0,
            path: path_str.clone(),
            created_at: now,
            updated_at: now,
        };
        let id = folder.insert(&self.db)?;
        Ok(Folder { id, ..folder })
    }

    pub fn compute_file_hash(path: &Path) -> Result<String> {
        let mut file = fs::File::open(path)
            .with_context(|| format!("failed to open file for hashing: {:?}", path))?;
        let mut hasher = Hasher::new();
        let mut buf = [0u8; 8192];

        loop {
            let read = file.read(&mut buf)?;
            if read == 0 {
                break;
            }
            hasher.update(&buf[..read]);
        }

        Ok(hasher.finalize().to_hex().to_string())
    }

    fn load_image_for_thumbnail(path: &Path) -> Result<DynamicImage> {
        image::open(path).with_context(|| format!("failed to decode image {:?}", path))
    }

    fn thumbnail_bytes(img: &DynamicImage, max_dim: u32) -> Result<Vec<u8>> {
        let thumb = img.thumbnail(max_dim, max_dim);
        let mut out = Vec::new();
        {
            let mut cursor = Cursor::new(&mut out);
            thumb
                .write_to(&mut cursor, ImageOutputFormat::Png)
                .context("failed to encode thumbnail as PNG")?;
        }
        Ok(out)
    }

    fn extract_exif_metadata(&self, _path: &Path) -> Result<Option<Value>> {
        // TODO: Integrate EXIF extraction (e.g. via rexiv2) and populate metadata_json.
        Ok(None)
    }

    fn modified_time(metadata: &fs::Metadata) -> Option<DateTime<Utc>> {
        metadata.modified().ok().map(DateTime::<Utc>::from)
    }

    fn parent_path(path: &Path) -> PathBuf {
        path.parent().map(Path::to_path_buf).unwrap_or_else(|| {
            // Store images with no parent into a pseudo-root bucket.
            PathBuf::from("/")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::initialize_schema;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    fn service_with_fresh_db() -> CatalogService {
        let db = CatalogDb::in_memory().unwrap();
        initialize_schema(db.conn()).unwrap();
        CatalogService::new(db)
    }

    fn write_temp_image(name: &str) -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::from_secs(0))
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("catalog_service_{suffix}"));
        fs::create_dir_all(&dir).unwrap();
        let file_path = dir.join(name);
        fs::write(&file_path, b"dummy").unwrap();
        file_path
    }

    #[test]
    fn import_image_creates_records() {
        let service = service_with_fresh_db();
        let path = write_temp_image("catalog_service_import.dng");

        let image = service.import_image(&path).expect("import failed");
        assert_eq!(image.filename, "catalog_service_import.dng");
        assert_eq!(service.count_images().unwrap(), 1);

        fs::remove_file(path).ok();
    }

    #[test]
    fn add_and_remove_keywords() {
        let service = service_with_fresh_db();
        let now = Utc::now();

        let folder = Folder {
            id: 0,
            path: "/keywords".into(),
            created_at: now,
            updated_at: now,
        };
        let folder_id = folder.insert(&service.db).unwrap();
        let image = Image {
            id: 0,
            folder_id,
            filename: "img.dng".into(),
            original_path: "/keywords/img.dng".into(),
            sidecar_path: None,
            sidecar_hash: None,
            filesize: None,
            file_hash: None,
            file_modified_at: None,
            imported_at: now,
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
            metadata_json: None,
            created_at: now,
            updated_at: now,
        };
        let image_id = image.insert(&service.db).unwrap();

        service
            .add_keyword_to_image(image_id, "sky")
            .expect("assign keyword");
        let keywords = ImageKeyword::list_keywords_for_image(&service.db, image_id).unwrap();
        assert_eq!(keywords.len(), 1);

        service
            .remove_keyword_from_image(image_id, "sky")
            .expect("remove keyword");
        let keywords = ImageKeyword::list_keywords_for_image(&service.db, image_id).unwrap();
        assert!(keywords.is_empty());
    }

    #[test]
    fn collections_and_listing() {
        let service = service_with_fresh_db();
        let now = Utc::now();

        let folder = Folder {
            id: 0,
            path: "/collections".into(),
            created_at: now,
            updated_at: now,
        };
        let folder_id = folder.insert(&service.db).unwrap();
        let image = Image {
            id: 0,
            folder_id,
            filename: "img.dng".into(),
            original_path: "/collections/img.dng".into(),
            sidecar_path: None,
            sidecar_hash: None,
            filesize: None,
            file_hash: None,
            file_modified_at: None,
            imported_at: now,
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
            metadata_json: None,
            created_at: now,
            updated_at: now,
        };
        let image_id = image.insert(&service.db).unwrap();

        let collection = service.create_collection("Favorites").unwrap();
        service
            .add_image_to_collection(collection.id, image_id)
            .unwrap();

        let images = service
            .list_images_in_collection(collection.id)
            .expect("list images");
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].id, image_id);
    }

    #[test]
    fn aggregates_and_ratings() {
        let service = service_with_fresh_db();
        let now = Utc::now();

        let folder = Folder {
            id: 0,
            path: "/aggregate".into(),
            created_at: now,
            updated_at: now,
        };
        let folder_id = folder.insert(&service.db).unwrap();

        let first = Image {
            id: 0,
            folder_id,
            filename: "one.dng".into(),
            original_path: "/aggregate/one.dng".into(),
            sidecar_path: None,
            sidecar_hash: None,
            filesize: None,
            file_hash: None,
            file_modified_at: None,
            imported_at: now,
            captured_at: None,
            camera_make: Some("ACME".into()),
            camera_model: Some("A1".into()),
            lens_model: None,
            focal_length: None,
            aperture: None,
            shutter_speed: None,
            iso: None,
            orientation: None,
            gps_latitude: None,
            gps_longitude: None,
            gps_altitude: None,
            rating: Some(5),
            flag: None,
            color_label: None,
            metadata_json: None,
            created_at: now,
            updated_at: now,
        };
        first.insert(&service.db).unwrap();

        let second = Image {
            rating: Some(3),
            camera_model: Some("A1".into()),
            original_path: "/aggregate/two.dng".into(),
            filename: "two.dng".into(),
            ..first.clone()
        };
        second.insert(&service.db).unwrap();

        let counts = service.count_by_camera().unwrap();
        assert_eq!(counts.get("A1"), Some(&2));

        let rated = service.images_with_rating(5).unwrap();
        assert_eq!(rated.len(), 1);
        assert_eq!(rated[0].filename, "one.dng");
    }

    #[test]
    fn apply_edits_upserts() {
        let service = service_with_fresh_db();
        let now = Utc::now();

        let folder = Folder {
            id: 0,
            path: "/edits".into(),
            created_at: now,
            updated_at: now,
        };
        let folder_id = folder.insert(&service.db).unwrap();
        let image = Image {
            id: 0,
            folder_id,
            filename: "img.dng".into(),
            original_path: "/edits/img.dng".into(),
            sidecar_path: None,
            sidecar_hash: None,
            filesize: None,
            file_hash: None,
            file_modified_at: None,
            imported_at: now,
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
            metadata_json: None,
            created_at: now,
            updated_at: now,
        };
        let image_id = image.insert(&service.db).unwrap();

        let edits = Edits {
            id: 0,
            image_id,
            exposure: Some(0.25),
            contrast: Some(0.5),
            highlights: None,
            shadows: None,
            whites: None,
            blacks: None,
            vibrance: None,
            saturation: None,
            temperature: None,
            tint: None,
            texture: None,
            clarity: None,
            dehaze: None,
            parametric_curve_json: None,
            color_grading_json: None,
            crop_json: None,
            masking_json: None,
            updated_at: None,
        };

        service.apply_edits(image_id, edits.clone()).unwrap();

        let first_exposure: f64 = query_optional(
            &service.db,
            "SELECT exposure FROM edits WHERE image_id = ?1",
            params![image_id],
            |row| Ok(row.get::<_, f64>(0)?),
        )
        .unwrap()
        .unwrap();
        assert!((first_exposure - 0.25).abs() < f64::EPSILON);

        let edits = Edits {
            exposure: Some(0.75),
            ..edits
        };
        service.apply_edits(image_id, edits).unwrap();

        let updated_exposure: f64 = query_optional(
            &service.db,
            "SELECT exposure FROM edits WHERE image_id = ?1",
            params![image_id],
            |row| Ok(row.get::<_, f64>(0)?),
        )
        .unwrap()
        .unwrap();
        assert!((updated_exposure - 0.75).abs() < f64::EPSILON);
    }

    #[test]
    fn list_folders_and_images() {
        let service = service_with_fresh_db();
        let now = Utc::now();

        let folder = Folder {
            id: 0,
            path: "/list".into(),
            created_at: now,
            updated_at: now,
        };
        let folder_id = folder.insert(&service.db).unwrap();

        let image = Image {
            id: 0,
            folder_id,
            filename: "img.jpg".into(),
            original_path: "/list/img.jpg".into(),
            sidecar_path: None,
            sidecar_hash: None,
            filesize: Some(10),
            file_hash: None,
            file_modified_at: None,
            imported_at: now,
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
            metadata_json: None,
            created_at: now,
            updated_at: now,
        };
        let image_id = image.insert(&service.db).unwrap();

        let folders = service.list_folders().unwrap();
        assert_eq!(folders.len(), 1);
        assert_eq!(folders[0].path, "/list");

        let images = service.list_images_in_folder(Path::new("/list")).unwrap();
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].id, image_id);
    }

    #[test]
    fn update_metadata_helpers() {
        let service = service_with_fresh_db();
        let now = Utc::now();

        let folder = Folder {
            id: 0,
            path: "/helpers".into(),
            created_at: now,
            updated_at: now,
        };
        let folder_id = folder.insert(&service.db).unwrap();
        let image = Image {
            id: 0,
            folder_id,
            filename: "img.dng".into(),
            original_path: "/helpers/img.dng".into(),
            sidecar_path: None,
            sidecar_hash: None,
            filesize: None,
            file_hash: None,
            file_modified_at: None,
            imported_at: now,
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
            metadata_json: None,
            created_at: now,
            updated_at: now,
        };
        let image_id = image.insert(&service.db).unwrap();

        service.update_rating(image_id, 4).unwrap();
        service
            .update_flag(image_id, Some("picked".into()))
            .unwrap();
        service
            .update_color_label(image_id, Some("Red".into()))
            .unwrap();
        service
            .update_keywords(image_id, &["sky".into(), "mountain".into()])
            .unwrap();

        let details = service.load_metadata(image_id).unwrap();
        assert_eq!(details.image.rating, Some(4));
        assert_eq!(details.image.flag.as_deref(), Some("picked"));
        assert_eq!(details.image.color_label.as_deref(), Some("red"));
        assert!(details.keywords.contains(&"sky".into()));
        assert!(details.keywords.contains(&"mountain".into()));
    }
}
