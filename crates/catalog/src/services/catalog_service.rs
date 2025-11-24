use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufReader, Cursor, Read};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use blake3::Hasher;
use chrono::{DateTime, NaiveDateTime, Utc};
use core_types::raw_jpeg::{extract_exif_segment, find_embedded_jpeg};
use exif::{Reader, Tag, Value as ExifValue};
use image::imageops::{overlay, FilterType};
use image::{DynamicImage, ImageBuffer, ImageOutputFormat, RgbaImage};
use jpeg_decoder::{Decoder as JpegDecoder, PixelFormat};
use rawloader::{decode_file as decode_raw_file, Orientation as RawOrientation};
use rusqlite::params;
use serde_json::{Map, Value};

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

    pub fn list_images_recursively(&self, folder_path: &Path) -> Result<Vec<Image>> {
        let normalized = folder_path.to_string_lossy().to_string();
        let mut prefix = normalized.clone();
        if !normalized.ends_with(std::path::MAIN_SEPARATOR)
            && !normalized.ends_with('/')
            && !normalized.ends_with('\\')
        {
            prefix.push(std::path::MAIN_SEPARATOR);
        }
        prefix.push('%');

        query_all(
            &self.db,
            "SELECT
                i.id, i.folder_id, i.filename, i.original_path, i.sidecar_path, i.sidecar_hash, i.filesize,
                i.file_hash, i.file_modified_at, i.imported_at, i.captured_at, i.camera_make,
                i.camera_model, i.lens_model, i.focal_length, i.aperture, i.shutter_speed, i.iso,
                i.orientation, i.gps_latitude, i.gps_longitude, i.gps_altitude, i.rating, i.flag,
                i.color_label, i.metadata_json, i.created_at, i.updated_at
             FROM images i
             INNER JOIN folders f ON f.id = i.folder_id
             WHERE f.path = ?1 OR f.path LIKE ?2
             ORDER BY i.captured_at IS NULL, i.captured_at",
            params![normalized, prefix],
            Image::from_row,
        )
        .context("failed to list images recursively")
    }

    pub fn list_all_photos(&self) -> Result<Vec<Image>> {
        query_all(
            &self.db,
            "SELECT
                id, folder_id, filename, original_path, sidecar_path, sidecar_hash, filesize,
                file_hash, file_modified_at, imported_at, captured_at, camera_make,
                camera_model, lens_model, focal_length, aperture, shutter_speed, iso,
                orientation, gps_latitude, gps_longitude, gps_altitude, rating, flag,
                color_label, metadata_json, created_at, updated_at
             FROM images
             ORDER BY captured_at IS NULL, captured_at",
            [],
            Image::from_row,
        )
        .context("failed to list all photos")
    }

    pub fn last_import_timestamp(&self) -> Result<Option<DateTime<Utc>>> {
        query_one(&self.db, "SELECT MAX(imported_at) FROM images", [], |row| {
            let raw: Option<String> = row.get(0)?;
            if let Some(raw) = raw {
                let parsed = DateTime::parse_from_rfc3339(&raw)
                    .map(|dt| dt.with_timezone(&Utc))
                    .with_context(|| format!("failed to parse imported_at timestamp {raw}"))?;
                Ok(Some(parsed))
            } else {
                Ok(None)
            }
        })
        .context("failed to read last import timestamp")
    }

    pub fn list_last_import(&self, since: Option<DateTime<Utc>>) -> Result<Vec<Image>> {
        let Some(cutoff) = since.or(self.last_import_timestamp()?) else {
            return Ok(Vec::new());
        };

        query_all(
            &self.db,
            "SELECT
                id, folder_id, filename, original_path, sidecar_path, sidecar_hash, filesize,
                file_hash, file_modified_at, imported_at, captured_at, camera_make,
                camera_model, lens_model, focal_length, aperture, shutter_speed, iso,
                orientation, gps_latitude, gps_longitude, gps_altitude, rating, flag,
                color_label, metadata_json, created_at, updated_at
             FROM images
             WHERE imported_at >= ?1
             ORDER BY captured_at IS NULL, captured_at",
            params![to_rfc3339(cutoff)],
            Image::from_row,
        )
        .context("failed to list last import images")
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

    pub fn update_flag(&self, image_id: i64, flag: &str) -> Result<()> {
        let normalized = flag.trim();
        let normalized = match normalized.to_ascii_lowercase().as_str() {
            "" | "none" => None,
            other => Some(other.to_string()),
        };

        self.db
            .execute(
                "UPDATE images SET flag = ?1, updated_at = ?2 WHERE id = ?3",
                params![normalized, to_rfc3339(Utc::now()), image_id],
            )
            .with_context(|| format!("failed to update flag for image_id={image_id}"))?;
        Ok(())
    }

    pub fn update_color_label(&self, image_id: i64, label: &str) -> Result<()> {
        let normalized = label.trim();
        let normalized = match normalized.to_ascii_lowercase().as_str() {
            "" | "none" => None,
            other => Some(other.to_string()),
        };
        self.db
            .execute(
                "UPDATE images SET color_label = ?1, updated_at = ?2 WHERE id = ?3",
                params![normalized, to_rfc3339(Utc::now()), image_id],
            )
            .with_context(|| format!("failed to update color label for image_id={image_id}"))?;
        Ok(())
    }

    pub fn update_sidecar_path(&self, image_id: i64, sidecar_path: Option<&Path>) -> Result<()> {
        let normalized = sidecar_path.map(|p| p.to_string_lossy().to_string());
        self.db
            .execute(
                "UPDATE images SET sidecar_path = ?1, updated_at = ?2 WHERE id = ?3",
                params![normalized, to_rfc3339(Utc::now()), image_id],
            )
            .with_context(|| format!("failed to update sidecar path for image_id={image_id}"))?;
        Ok(())
    }

    pub fn import_image(&self, path: &Path) -> Result<Image> {
        let now = Utc::now();
        self.import_image_at(path, now)
    }

    pub fn import_image_at(&self, path: &Path, imported_at: DateTime<Utc>) -> Result<Image> {
        let metadata = fs::metadata(path)
            .with_context(|| format!("failed to read file metadata for {:?}", path))?;
        let folder_path = Self::parent_path(path);
        let folder = self.ensure_folder(&folder_path)?;
        let file_hash = Self::compute_file_hash(path)
            .with_context(|| format!("failed to hash file {:?}", path))?;

        let exif_data = self.extract_exif_metadata(path)?;
        let metadata_json = match exif_data
            .as_ref()
            .and_then(|data| data.metadata_json.clone())
        {
            Some(json) => Some(json),
            None => self.scan_raw_metadata(path)?,
        };

        let filename = path
            .file_name()
            .and_then(|s| s.to_str())
            .context("image path is missing a valid filename")?
            .to_string();

        let now = Utc::now();
        let mut image = Image {
            id: 0,
            folder_id: folder.id,
            filename,
            original_path: path.to_string_lossy().to_string(),
            sidecar_path: None,
            sidecar_hash: None,
            filesize: Some(metadata.len() as i64),
            file_hash: Some(file_hash),
            file_modified_at: Self::modified_time(&metadata),
            imported_at,
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

        if let Some(exif) = exif_data {
            image.captured_at = exif.captured_at;
            image.camera_make = exif.camera_make;
            image.camera_model = exif.camera_model;
            image.lens_model = exif.lens_model;
            image.focal_length = exif.focal_length;
            image.aperture = exif.aperture;
            image.shutter_speed = exif.shutter_speed;
            image.iso = exif.iso;
            image.orientation = exif.orientation;
            image.gps_latitude = exif.gps_latitude;
            image.gps_longitude = exif.gps_longitude;
            image.gps_altitude = exif.gps_altitude;
        }

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
        match catch_unwind(AssertUnwindSafe(|| image::open(path))) {
            Ok(Ok(img)) => Ok(img),
            Ok(Err(open_err)) => {
                if let Some(bytes) = find_embedded_jpeg(path)? {
                    Self::decode_embedded_jpeg(&bytes).with_context(|| {
                        format!("failed to decode embedded JPEG preview for {:?}", path)
                    })
                } else {
                    Err(anyhow!(open_err)).with_context(|| {
                        format!(
                            "failed to decode image and no embedded preview found: {:?}",
                            path
                        )
                    })
                }
            }
            Err(_) => Err(anyhow!("image::open panicked for {:?}", path)),
        }
    }

    fn decode_embedded_jpeg(bytes: &[u8]) -> Result<DynamicImage> {
        let mut decoder = JpegDecoder::new(Cursor::new(bytes));
        let pixels = decoder
            .decode()
            .map_err(|err| anyhow!("embedded JPEG decode failed: {err}"))?;
        let info = decoder
            .info()
            .ok_or_else(|| anyhow!("embedded JPEG metadata missing"))?;
        let dyn_img = match info.pixel_format {
            PixelFormat::L8 => {
                let buffer = ImageBuffer::from_vec(info.width as u32, info.height as u32, pixels)
                    .ok_or_else(|| anyhow!("embedded JPEG luma buffer size mismatch"))?;
                DynamicImage::ImageLuma8(buffer)
            }
            PixelFormat::RGB24 => {
                let buffer = ImageBuffer::from_vec(info.width as u32, info.height as u32, pixels)
                    .ok_or_else(|| anyhow!("embedded JPEG RGB buffer size mismatch"))?;
                DynamicImage::ImageRgb8(buffer)
            }
            PixelFormat::CMYK32 => {
                let mut rgb = Vec::with_capacity((info.width * info.height * 3) as usize);
                for chunk in pixels.chunks_exact(4) {
                    let c = chunk[0] as f32 / 255.0;
                    let m = chunk[1] as f32 / 255.0;
                    let y = chunk[2] as f32 / 255.0;
                    let k = chunk[3] as f32 / 255.0;
                    let r = (1.0 - (c * (1.0 - k) + k)) * 255.0;
                    let g = (1.0 - (m * (1.0 - k) + k)) * 255.0;
                    let b = (1.0 - (y * (1.0 - k) + k)) * 255.0;
                    rgb.push(r.clamp(0.0, 255.0) as u8);
                    rgb.push(g.clamp(0.0, 255.0) as u8);
                    rgb.push(b.clamp(0.0, 255.0) as u8);
                }
                let buffer = ImageBuffer::from_vec(info.width as u32, info.height as u32, rgb)
                    .ok_or_else(|| anyhow!("embedded JPEG CMYK buffer size mismatch"))?;
                DynamicImage::ImageRgb8(buffer)
            }
            other => {
                return Err(anyhow!(
                    "unsupported embedded JPEG pixel format: {:?}",
                    other
                ))
            }
        };
        Ok(dyn_img)
    }

    fn extract_raw_metadata(&self, path: &Path) -> Result<Option<ExtractedExif>> {
        match decode_raw_file(path) {
            Ok(raw) => {
                let mut summary = ExtractedExif::default();
                if let Some(make) = sanitize_non_empty(&raw.clean_make)
                    .or_else(|| sanitize_non_empty(&raw.make))
                {
                    summary.camera_make = Some(make);
                }
                if let Some(model) = sanitize_non_empty(&raw.clean_model)
                    .or_else(|| sanitize_non_empty(&raw.model))
                {
                    summary.camera_model = Some(model);
                }
                summary.orientation = raw_orientation_to_tag(raw.orientation);
                Ok(Some(summary))
            }
            Err(err) => {
                eprintln!(
                    "RAW metadata extraction failed for {}: {err}",
                    path.display()
                );
                Ok(None)
            }
        }
    }

    fn thumbnail_bytes(img: &DynamicImage, max_dim: u32) -> Result<Vec<u8>> {
        let thumb = Self::letterboxed_thumbnail(img, max_dim);
        let mut out = Vec::new();
        {
            let mut cursor = Cursor::new(&mut out);
            DynamicImage::ImageRgba8(thumb)
                .write_to(&mut cursor, ImageOutputFormat::Png)
                .context("failed to encode thumbnail as PNG")?;
        }
        Ok(out)
    }

    fn letterboxed_thumbnail(img: &DynamicImage, max_dim: u32) -> RgbaImage {
        let resized = img
            .resize(max_dim, max_dim, FilterType::Lanczos3)
            .to_rgba8();
        let (w, h) = resized.dimensions();
        if w == max_dim && h == max_dim {
            return resized;
        }

        let mut canvas = RgbaImage::from_pixel(max_dim, max_dim, image::Rgba([16, 16, 16, 255]));
        let offset_x = (max_dim - w) / 2;
        let offset_y = (max_dim - h) / 2;
        overlay(&mut canvas, &resized, offset_x.into(), offset_y.into());
        canvas
    }

    fn extract_exif_metadata(&self, path: &Path) -> Result<Option<ExtractedExif>> {
        let file = fs::File::open(path)
            .with_context(|| format!("failed to open {} for EXIF parsing", path.display()))?;
        let mut reader = BufReader::new(file);
        let exif = match Reader::new().read_from_container(&mut reader) {
            Ok(exif) => exif,
            Err(err) => {
                if let Some(bytes) = find_embedded_jpeg(path)
                    .with_context(|| format!("failed to locate embedded preview in {}", path.display()))?
                {
                    if let Some(segment) = extract_exif_segment(&bytes) {
                        match Reader::new().read_raw(segment) {
                            Ok(exif) => exif,
                            Err(inner) => {
                                eprintln!(
                                    "EXIF parse failed for {} (embedded JPEG): {inner}",
                                    path.display()
                                );
                                return self.extract_raw_metadata(path);
                            }
                        }
                    } else {
                        let mut cursor = Cursor::new(bytes);
                        match Reader::new().read_from_container(&mut cursor) {
                            Ok(exif) => exif,
                            Err(inner) => {
                                eprintln!(
                                    "Embedded preview missing EXIF segment for {}: {err}; fallback parse error: {inner}",
                                    path.display()
                                );
                                return self.extract_raw_metadata(path);
                            }
                        }
                    }
                } else {
                    eprintln!("No EXIF data available for {}: {err}", path.display());
                    return self.extract_raw_metadata(path);
                }
            }
        };

        let mut summary = ExtractedExif::default();
        let mut json = Map::new();
        let mut gps_lat_value: Option<[f64; 3]> = None;
        let mut gps_lat_ref: Option<String> = None;
        let mut gps_lon_value: Option<[f64; 3]> = None;
        let mut gps_lon_ref: Option<String> = None;
        let mut gps_alt_value: Option<f64> = None;
        let mut gps_alt_ref: Option<u8> = None;

        for field in exif.fields() {
            let key = format!("{:?}.{:?}", field.ifd_num, field.tag);
            json.insert(
                key,
                Value::String(field.display_value().with_unit(&exif).to_string()),
            );

            match field.tag {
                Tag::DateTimeOriginal | Tag::DateTimeDigitized | Tag::DateTime => {
                    if summary.captured_at.is_none() {
                        summary.captured_at = parse_exif_datetime(&field.value);
                    }
                }
                Tag::Make => {
                    if summary.camera_make.is_none() {
                        summary.camera_make = exif_string(&field.value);
                    }
                }
                Tag::Model => {
                    if summary.camera_model.is_none() {
                        summary.camera_model = exif_string(&field.value);
                    }
                }
                Tag::LensModel => {
                    if summary.lens_model.is_none() {
                        summary.lens_model = exif_string(&field.value);
                    }
                }
                Tag::FocalLength => {
                    if summary.focal_length.is_none() {
                        summary.focal_length = rational_value(&field.value);
                    }
                }
                Tag::FNumber => {
                    if summary.aperture.is_none() {
                        summary.aperture = rational_value(&field.value);
                    }
                }
                Tag::ExposureTime => {
                    if summary.shutter_speed.is_none() {
                        summary.shutter_speed = rational_value(&field.value);
                    }
                }
                Tag::PhotographicSensitivity | Tag::ISOSpeed => {
                    if summary.iso.is_none() {
                        summary.iso = int_value(&field.value);
                    }
                }
                Tag::Orientation => {
                    if summary.orientation.is_none() {
                        summary.orientation = int_value(&field.value);
                    }
                }
                Tag::GPSLatitude => {
                    if let ExifValue::Rational(values) = &field.value {
                        if values.len() >= 3 {
                            gps_lat_value = Some([
                                values[0].to_f64(),
                                values[1].to_f64(),
                                values[2].to_f64(),
                            ]);
                        }
                    }
                }
                Tag::GPSLatitudeRef => {
                    gps_lat_ref = exif_string(&field.value);
                }
                Tag::GPSLongitude => {
                    if let ExifValue::Rational(values) = &field.value {
                        if values.len() >= 3 {
                            gps_lon_value = Some([
                                values[0].to_f64(),
                                values[1].to_f64(),
                                values[2].to_f64(),
                            ]);
                        }
                    }
                }
                Tag::GPSLongitudeRef => {
                    gps_lon_ref = exif_string(&field.value);
                }
                Tag::GPSAltitude => {
                    if let ExifValue::Rational(values) = &field.value {
                        if let Some(raw) = values.get(0) {
                            gps_alt_value = Some(raw.to_f64());
                        }
                    }
                }
                Tag::GPSAltitudeRef => {
                    if let Some(value) = int_value(&field.value) {
                        gps_alt_ref = Some(value as u8);
                    }
                }
                _ => {}
            }
        }

        if summary.camera_make.is_none()
            || summary.camera_model.is_none()
            || summary.orientation.is_none()
        {
            if let Some(raw_meta) = self.extract_raw_metadata(path)? {
                if summary.camera_make.is_none() {
                    summary.camera_make = raw_meta.camera_make;
                }
                if summary.camera_model.is_none() {
                    summary.camera_model = raw_meta.camera_model;
                }
                if summary.orientation.is_none() {
                    summary.orientation = raw_meta.orientation;
                }
            }
        }

        summary.gps_latitude = gps_coordinate(gps_lat_value, gps_lat_ref.as_deref());
        summary.gps_longitude = gps_coordinate(gps_lon_value, gps_lon_ref.as_deref());
        summary.gps_altitude = gps_altitude(gps_alt_value, gps_alt_ref);

        if !json.is_empty() {
            summary.metadata_json = Some(Value::Object(json));
        }

        Ok(Some(summary))
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

    pub fn remove_image(&self, image_id: i64) -> Result<()> {
        ImageKeyword::delete_for_image(&self.db, image_id)
            .context("failed to delete image keywords")?;
        Thumbnail::delete(&self.db, image_id).context("failed to delete image thumbnails")?;
        Preview::delete(&self.db, image_id).context("failed to delete image previews")?;
        Image::delete(&self.db, image_id).context("failed to delete image")?;
        Ok(())
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
        service.update_flag(image_id, "picked").unwrap();
        service.update_color_label(image_id, "Red").unwrap();
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

#[derive(Default)]
struct ExtractedExif {
    metadata_json: Option<Value>,
    captured_at: Option<DateTime<Utc>>,
    camera_make: Option<String>,
    camera_model: Option<String>,
    lens_model: Option<String>,
    focal_length: Option<f64>,
    aperture: Option<f64>,
    shutter_speed: Option<f64>,
    iso: Option<i64>,
    orientation: Option<i64>,
    gps_latitude: Option<f64>,
    gps_longitude: Option<f64>,
    gps_altitude: Option<f64>,
}

fn exif_string(value: &ExifValue) -> Option<String> {
    match value {
        ExifValue::Ascii(values) => values
            .get(0)
            .and_then(|raw| std::str::from_utf8(raw).ok())
            .map(|s| s.trim_matches('\u{0}').trim().to_string())
            .filter(|s| !s.is_empty()),
        _ => None,
    }
}

fn parse_exif_datetime(value: &ExifValue) -> Option<DateTime<Utc>> {
    let raw = exif_string(value)?;
    NaiveDateTime::parse_from_str(raw.trim(), "%Y:%m:%d %H:%M:%S")
        .ok()
        .map(|naive| naive.and_utc())
}

fn rational_value(value: &ExifValue) -> Option<f64> {
    match value {
        ExifValue::Rational(values) if !values.is_empty() => Some(values[0].to_f64()),
        ExifValue::SRational(values) if !values.is_empty() => Some(values[0].to_f64()),
        _ => None,
    }
}

fn int_value(value: &ExifValue) -> Option<i64> {
    match value {
        ExifValue::Byte(values) => values.get(0).map(|v| *v as i64),
        ExifValue::Short(values) => values.get(0).map(|v| *v as i64),
        ExifValue::Long(values) => values.get(0).map(|v| *v as i64),
        ExifValue::SByte(values) => values.get(0).map(|v| *v as i64),
        ExifValue::SShort(values) => values.get(0).map(|v| *v as i64),
        ExifValue::SLong(values) => values.get(0).map(|v| *v as i64),
        _ => None,
    }
}

fn gps_coordinate(values: Option<[f64; 3]>, reference: Option<&str>) -> Option<f64> {
    let components = values?;
    let degrees = components[0];
    let minutes = components[1];
    let seconds = components[2];
    let mut sign = 1.0;
    if let Some(reference) = reference {
        if matches!(
            reference.trim().to_ascii_uppercase().as_str(),
            "S" | "W"
        ) {
            sign = -1.0;
        }
    }
    Some(sign * (degrees + minutes / 60.0 + seconds / 3600.0))
}

fn gps_altitude(value: Option<f64>, reference: Option<u8>) -> Option<f64> {
    let mut result = value?;
    if matches!(reference, Some(1)) {
        result = -result;
    }
    Some(result)
}

fn sanitize_non_empty(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn raw_orientation_to_tag(orientation: RawOrientation) -> Option<i64> {
    match orientation {
        RawOrientation::Normal => Some(1),
        RawOrientation::HorizontalFlip => Some(2),
        RawOrientation::Rotate180 => Some(3),
        RawOrientation::VerticalFlip => Some(4),
        RawOrientation::Transpose => Some(5),
        RawOrientation::Rotate90 => Some(6),
        RawOrientation::Transverse => Some(7),
        RawOrientation::Rotate270 => Some(8),
        RawOrientation::Unknown => None,
    }
}
