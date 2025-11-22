use crate::db::{
    parse_datetime, query_all, query_one, query_optional, to_rfc3339, DbHandle, DbResult, Image,
};
use anyhow::Context;
use chrono::{DateTime, Utc};
use rusqlite::params;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Collection {
    pub id: i64,
    pub name: String,
    pub parent_id: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Collection {
    pub fn insert<H: DbHandle>(&self, db: &H) -> DbResult<i64> {
        db.execute(
            "INSERT INTO collections (name, parent_id, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                self.name,
                self.parent_id,
                to_rfc3339(self.created_at),
                to_rfc3339(self.updated_at)
            ],
        )
        .with_context(|| format!("failed to insert collection {}", self.name))?;
        Ok(db.last_insert_rowid())
    }

    pub fn load<H: DbHandle>(db: &H, id: i64) -> DbResult<Self> {
        query_one(
            db,
            "SELECT id, name, parent_id, created_at, updated_at FROM collections WHERE id = ?1",
            params![id],
            Collection::from_row,
        )
        .with_context(|| format!("failed to load collection id={id}"))
    }

    pub fn load_all<H: DbHandle>(db: &H) -> DbResult<Vec<Self>> {
        query_all(
            db,
            "SELECT id, name, parent_id, created_at, updated_at FROM collections ORDER BY name",
            [],
            Collection::from_row,
        )
    }

    pub fn update<H: DbHandle>(&self, db: &H) -> DbResult<()> {
        db.execute(
            "UPDATE collections SET name = ?1, parent_id = ?2, created_at = ?3, updated_at = ?4 WHERE id = ?5",
            params![
                self.name,
                self.parent_id,
                to_rfc3339(self.created_at),
                to_rfc3339(self.updated_at),
                self.id
            ],
        )
        .with_context(|| format!("failed to update collection id={}", self.id))?;
        Ok(())
    }

    pub fn delete<H: DbHandle>(db: &H, id: i64) -> DbResult<()> {
        db.execute("DELETE FROM collections WHERE id = ?1", params![id])
            .with_context(|| format!("failed to delete collection id={id}"))?;
        Ok(())
    }

    pub fn add_image<H: DbHandle>(db: &H, collection_id: i64, image_id: i64) -> DbResult<()> {
        let next_position: i64 = query_optional(
            db,
            "SELECT MAX(position) FROM collection_images WHERE collection_id = ?1",
            params![collection_id],
            |row| Ok(row.get::<_, Option<i64>>(0)?.unwrap_or(0)),
        )?
        .unwrap_or(0)
            + 1;

        db.execute(
            "INSERT OR REPLACE INTO collection_images (collection_id, image_id, position, added_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                collection_id,
                image_id,
                next_position,
                to_rfc3339(Utc::now())
            ],
        )
        .with_context(|| {
            format!(
                "failed to add image {} to collection {}",
                image_id, collection_id
            )
        })?;
        Ok(())
    }

    pub fn list_images<H: DbHandle>(db: &H, collection_id: i64) -> DbResult<Vec<Image>> {
        query_all(
            db,
            "SELECT
                i.id, i.folder_id, i.filename, i.original_path, i.sidecar_path, i.sidecar_hash,
                i.filesize, i.file_hash, i.file_modified_at, i.imported_at, i.captured_at,
                i.camera_make, i.camera_model, i.lens_model, i.focal_length, i.aperture,
                i.shutter_speed, i.iso, i.orientation, i.gps_latitude, i.gps_longitude,
                i.gps_altitude, i.rating, i.flag, i.color_label, i.metadata_json,
                i.created_at, i.updated_at
             FROM images i
             INNER JOIN collection_images ci ON ci.image_id = i.id
             WHERE ci.collection_id = ?1
             ORDER BY ci.position",
            params![collection_id],
            Image::from_row,
        )
    }

    fn from_row(row: &rusqlite::Row<'_>) -> DbResult<Self> {
        Ok(Self {
            id: row.get(0)?,
            name: row.get(1)?,
            parent_id: row.get(2)?,
            created_at: parse_datetime(row.get::<_, String>(3)?, "created_at")?,
            updated_at: parse_datetime(row.get::<_, String>(4)?, "updated_at")?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Folder;
    use crate::schema::initialize_schema;

    #[test]
    fn add_and_list_images_in_collection() {
        let db = crate::db::CatalogDb::in_memory().unwrap();
        initialize_schema(db.conn()).unwrap();

        let folder = Folder {
            id: 0,
            path: "/photos".into(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let folder_id = folder.insert(&db).unwrap();

        let image = Image {
            id: 0,
            folder_id,
            filename: "img.dng".into(),
            original_path: "/photos/img.dng".into(),
            sidecar_path: None,
            sidecar_hash: None,
            filesize: None,
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
            metadata_json: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let image_id = image.insert(&db).unwrap();

        let collection = Collection {
            id: 0,
            name: "Favorites".into(),
            parent_id: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let collection_id = collection.insert(&db).unwrap();

        Collection::add_image(&db, collection_id, image_id).unwrap();
        let images = Collection::list_images(&db, collection_id).unwrap();

        assert_eq!(images.len(), 1);
        assert_eq!(images[0].id, image_id);
    }
}
