use crate::db::{
    from_json, parse_datetime, parse_datetime_opt, query_all, query_one, query_optional, to_json,
    to_rfc3339, to_rfc3339_opt, DbHandle, DbResult,
};
use anyhow::Context;
use chrono::{DateTime, Utc};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Image {
    pub id: i64,
    pub folder_id: i64,
    pub filename: String,
    pub original_path: String,
    pub sidecar_path: Option<String>,
    pub sidecar_hash: Option<String>,
    pub filesize: Option<i64>,
    pub file_hash: Option<String>,
    pub file_modified_at: Option<DateTime<Utc>>,
    pub imported_at: DateTime<Utc>,
    pub captured_at: Option<DateTime<Utc>>,
    pub camera_make: Option<String>,
    pub camera_model: Option<String>,
    pub lens_model: Option<String>,
    pub focal_length: Option<f64>,
    pub aperture: Option<f64>,
    pub shutter_speed: Option<f64>,
    pub iso: Option<i64>,
    pub orientation: Option<i64>,
    pub gps_latitude: Option<f64>,
    pub gps_longitude: Option<f64>,
    pub gps_altitude: Option<f64>,
    pub rating: Option<i64>,
    pub flag: Option<String>,
    pub color_label: Option<String>,
    pub metadata_json: Option<Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Image {
    pub fn insert<H: DbHandle>(&self, db: &H) -> DbResult<i64> {
        let metadata_json = self.metadata_json.as_ref().map(to_json).transpose()?;
        db.execute(
            "INSERT INTO images (
                folder_id, filename, original_path, sidecar_path, sidecar_hash, filesize,
                file_hash, file_modified_at, imported_at, captured_at, camera_make,
                camera_model, lens_model, focal_length, aperture, shutter_speed, iso,
                orientation, gps_latitude, gps_longitude, gps_altitude, rating, flag,
                color_label, metadata_json, created_at, updated_at
             ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6,
                ?7, ?8, ?9, ?10, ?11,
                ?12, ?13, ?14, ?15, ?16, ?17,
                ?18, ?19, ?20, ?21, ?22, ?23,
                ?24, ?25, ?26, ?27
             )",
            params![
                self.folder_id,
                self.filename,
                self.original_path,
                self.sidecar_path,
                self.sidecar_hash,
                self.filesize,
                self.file_hash,
                to_rfc3339_opt(self.file_modified_at),
                to_rfc3339(self.imported_at),
                to_rfc3339_opt(self.captured_at),
                self.camera_make,
                self.camera_model,
                self.lens_model,
                self.focal_length,
                self.aperture,
                self.shutter_speed,
                self.iso,
                self.orientation,
                self.gps_latitude,
                self.gps_longitude,
                self.gps_altitude,
                self.rating,
                self.flag,
                self.color_label,
                metadata_json,
                to_rfc3339(self.created_at),
                to_rfc3339(self.updated_at)
            ],
        )
        .with_context(|| format!("failed to insert image path={}", self.original_path))?;
        Ok(db.last_insert_rowid())
    }

    pub fn load<H: DbHandle>(db: &H, id: i64) -> DbResult<Self> {
        query_one(
            db,
            "SELECT
                id, folder_id, filename, original_path, sidecar_path, sidecar_hash, filesize,
                file_hash, file_modified_at, imported_at, captured_at, camera_make,
                camera_model, lens_model, focal_length, aperture, shutter_speed, iso,
                orientation, gps_latitude, gps_longitude, gps_altitude, rating, flag,
                color_label, metadata_json, created_at, updated_at
             FROM images WHERE id = ?1",
            params![id],
            Image::from_row,
        )
        .with_context(|| format!("failed to load image id={id}"))
    }

    pub fn load_all<H: DbHandle>(db: &H) -> DbResult<Vec<Self>> {
        query_all(
            db,
            "SELECT
                id, folder_id, filename, original_path, sidecar_path, sidecar_hash, filesize,
                file_hash, file_modified_at, imported_at, captured_at, camera_make,
                camera_model, lens_model, focal_length, aperture, shutter_speed, iso,
                orientation, gps_latitude, gps_longitude, gps_altitude, rating, flag,
                color_label, metadata_json, created_at, updated_at
             FROM images
             ORDER BY id",
            [],
            Image::from_row,
        )
    }

    pub fn update<H: DbHandle>(&self, db: &H) -> DbResult<()> {
        let metadata_json = self.metadata_json.as_ref().map(to_json).transpose()?;
        db.execute(
            "UPDATE images SET
                folder_id = ?1,
                filename = ?2,
                original_path = ?3,
                sidecar_path = ?4,
                sidecar_hash = ?5,
                filesize = ?6,
                file_hash = ?7,
                file_modified_at = ?8,
                imported_at = ?9,
                captured_at = ?10,
                camera_make = ?11,
                camera_model = ?12,
                lens_model = ?13,
                focal_length = ?14,
                aperture = ?15,
                shutter_speed = ?16,
                iso = ?17,
                orientation = ?18,
                gps_latitude = ?19,
                gps_longitude = ?20,
                gps_altitude = ?21,
                rating = ?22,
                flag = ?23,
                color_label = ?24,
                metadata_json = ?25,
                created_at = ?26,
                updated_at = ?27
             WHERE id = ?28",
            params![
                self.folder_id,
                self.filename,
                self.original_path,
                self.sidecar_path,
                self.sidecar_hash,
                self.filesize,
                self.file_hash,
                to_rfc3339_opt(self.file_modified_at),
                to_rfc3339(self.imported_at),
                to_rfc3339_opt(self.captured_at),
                self.camera_make,
                self.camera_model,
                self.lens_model,
                self.focal_length,
                self.aperture,
                self.shutter_speed,
                self.iso,
                self.orientation,
                self.gps_latitude,
                self.gps_longitude,
                self.gps_altitude,
                self.rating,
                self.flag,
                self.color_label,
                metadata_json,
                to_rfc3339(self.created_at),
                to_rfc3339(self.updated_at),
                self.id
            ],
        )
        .with_context(|| format!("failed to update image id={}", self.id))?;
        Ok(())
    }

    pub fn delete<H: DbHandle>(db: &H, id: i64) -> DbResult<()> {
        db.execute("DELETE FROM images WHERE id = ?1", params![id])
            .with_context(|| format!("failed to delete image id={id}"))?;
        Ok(())
    }

    pub fn find_by_hash<H: DbHandle>(db: &H, hash: &str) -> DbResult<Option<Self>> {
        query_optional(
            db,
            "SELECT
                id, folder_id, filename, original_path, sidecar_path, sidecar_hash, filesize,
                file_hash, file_modified_at, imported_at, captured_at, camera_make,
                camera_model, lens_model, focal_length, aperture, shutter_speed, iso,
                orientation, gps_latitude, gps_longitude, gps_altitude, rating, flag,
                color_label, metadata_json, created_at, updated_at
             FROM images WHERE file_hash = ?1",
            params![hash],
            Image::from_row,
        )
    }

    pub fn find_by_folder<H: DbHandle>(db: &H, folder_id: i64) -> DbResult<Vec<Self>> {
        query_all(
            db,
            "SELECT
                id, folder_id, filename, original_path, sidecar_path, sidecar_hash, filesize,
                file_hash, file_modified_at, imported_at, captured_at, camera_make,
                camera_model, lens_model, focal_length, aperture, shutter_speed, iso,
                orientation, gps_latitude, gps_longitude, gps_altitude, rating, flag,
                color_label, metadata_json, created_at, updated_at
             FROM images WHERE folder_id = ?1
             ORDER BY captured_at IS NULL, captured_at",
            params![folder_id],
            Image::from_row,
        )
    }

    pub fn search_by_keyword<H: DbHandle>(db: &H, keyword: &str) -> DbResult<Vec<Self>> {
        query_all(
            db,
            "SELECT DISTINCT
                i.id, i.folder_id, i.filename, i.original_path, i.sidecar_path, i.sidecar_hash,
                i.filesize, i.file_hash, i.file_modified_at, i.imported_at, i.captured_at,
                i.camera_make, i.camera_model, i.lens_model, i.focal_length, i.aperture,
                i.shutter_speed, i.iso, i.orientation, i.gps_latitude, i.gps_longitude,
                i.gps_altitude, i.rating, i.flag, i.color_label, i.metadata_json,
                i.created_at, i.updated_at
             FROM images i
             INNER JOIN image_keywords ik ON i.id = ik.image_id
             INNER JOIN keywords k ON k.id = ik.keyword_id
             WHERE k.keyword LIKE ?1
             ORDER BY i.captured_at IS NULL, i.captured_at",
            params![keyword],
            Image::from_row,
        )
    }

    pub(crate) fn from_row(row: &rusqlite::Row<'_>) -> DbResult<Self> {
        Ok(Self {
            id: row.get(0)?,
            folder_id: row.get(1)?,
            filename: row.get(2)?,
            original_path: row.get(3)?,
            sidecar_path: row.get(4)?,
            sidecar_hash: row.get(5)?,
            filesize: row.get(6)?,
            file_hash: row.get(7)?,
            file_modified_at: parse_datetime_opt(
                row.get::<_, Option<String>>(8)?,
                "file_modified_at",
            )?,
            imported_at: parse_datetime(row.get::<_, String>(9)?, "imported_at")?,
            captured_at: parse_datetime_opt(row.get::<_, Option<String>>(10)?, "captured_at")?,
            camera_make: row.get(11)?,
            camera_model: row.get(12)?,
            lens_model: row.get(13)?,
            focal_length: row.get(14)?,
            aperture: row.get(15)?,
            shutter_speed: row.get(16)?,
            iso: row.get(17)?,
            orientation: row.get(18)?,
            gps_latitude: row.get(19)?,
            gps_longitude: row.get(20)?,
            gps_altitude: row.get(21)?,
            rating: row.get(22)?,
            flag: row.get(23)?,
            color_label: row.get(24)?,
            metadata_json: {
                let raw: Option<String> = row.get(25)?;
                match raw {
                    Some(json) => Some(from_json(&json)?),
                    None => None,
                }
            },
            created_at: parse_datetime(row.get::<_, String>(26)?, "created_at")?,
            updated_at: parse_datetime(row.get::<_, String>(27)?, "updated_at")?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::CatalogDb;
    use crate::schema::initialize_schema;

    #[test]
    fn insert_and_load_image() {
        let db = CatalogDb::in_memory().unwrap();
        initialize_schema(db.conn()).unwrap();

        let folder = crate::db::Folder {
            id: 0,
            path: "/photos/2024".into(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let folder_id = folder.insert(&db).unwrap();

        let image = Image {
            id: 0,
            folder_id,
            filename: "img0001.dng".into(),
            original_path: "/photos/2024/img0001.dng".into(),
            sidecar_path: None,
            sidecar_hash: None,
            filesize: Some(42),
            file_hash: Some("abcd".into()),
            file_modified_at: None,
            imported_at: Utc::now(),
            captured_at: None,
            camera_make: Some("ACME".into()),
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
            rating: Some(5),
            flag: None,
            color_label: None,
            metadata_json: Some(serde_json::json!({"exposure":1.0})),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        let image_id = image.insert(&db).unwrap();
        let loaded = Image::load(&db, image_id).unwrap();
        assert_eq!(loaded.id, image_id);
        assert_eq!(loaded.folder_id, folder_id);
        assert_eq!(loaded.filename, "img0001.dng");
    }
}
