use crate::db::keywords::Keyword;
use crate::db::{query_all, query_one, to_rfc3339, DbHandle, DbResult};
use anyhow::Context;
use chrono::{DateTime, Utc};
use rusqlite::params;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageKeyword {
    pub image_id: i64,
    pub keyword_id: i64,
    pub assigned_at: DateTime<Utc>,
}

impl ImageKeyword {
    pub fn insert<H: DbHandle>(&self, db: &H) -> DbResult<i64> {
        db.execute(
            "INSERT INTO image_keywords (image_id, keyword_id, assigned_at)
             VALUES (?1, ?2, ?3)",
            params![self.image_id, self.keyword_id, to_rfc3339(self.assigned_at)],
        )
        .with_context(|| {
            format!(
                "failed to insert image_keyword image_id={} keyword_id={}",
                self.image_id, self.keyword_id
            )
        })?;
        Ok(db.last_insert_rowid())
    }

    pub fn load<H: DbHandle>(db: &H, image_id: i64, keyword_id: i64) -> DbResult<Self> {
        query_one(
            db,
            "SELECT image_id, keyword_id, assigned_at
             FROM image_keywords WHERE image_id = ?1 AND keyword_id = ?2",
            params![image_id, keyword_id],
            ImageKeyword::from_row,
        )
        .with_context(|| {
            format!(
                "failed to load image_keywords image_id={} keyword_id={}",
                image_id, keyword_id
            )
        })
    }

    pub fn load_all<H: DbHandle>(db: &H) -> DbResult<Vec<Self>> {
        query_all(
            db,
            "SELECT image_id, keyword_id, assigned_at FROM image_keywords ORDER BY assigned_at DESC",
            [],
            ImageKeyword::from_row,
        )
    }

    pub fn update<H: DbHandle>(&self, db: &H) -> DbResult<()> {
        db.execute(
            "UPDATE image_keywords
             SET assigned_at = ?1, image_id = ?2, keyword_id = ?3
             WHERE image_id = ?4 AND keyword_id = ?5",
            params![
                to_rfc3339(self.assigned_at),
                self.image_id,
                self.keyword_id,
                self.image_id,
                self.keyword_id
            ],
        )
        .with_context(|| {
            format!(
                "failed to update image_keywords image_id={} keyword_id={}",
                self.image_id, self.keyword_id
            )
        })?;
        Ok(())
    }

    pub fn delete<H: DbHandle>(db: &H, image_id: i64, keyword_id: i64) -> DbResult<()> {
        db.execute(
            "DELETE FROM image_keywords WHERE image_id = ?1 AND keyword_id = ?2",
            params![image_id, keyword_id],
        )
        .with_context(|| {
            format!(
                "failed to delete image_keywords image_id={} keyword_id={}",
                image_id, keyword_id
            )
        })?;
        Ok(())
    }

    pub fn list_keywords_for_image<H: DbHandle>(db: &H, image_id: i64) -> DbResult<Vec<Keyword>> {
        query_all(
            db,
            "SELECT k.id, k.keyword
             FROM keywords k
             INNER JOIN image_keywords ik ON ik.keyword_id = k.id
             WHERE ik.image_id = ?1
             ORDER BY k.keyword",
            params![image_id],
            Keyword::from_row,
        )
    }

    fn from_row(row: &rusqlite::Row<'_>) -> DbResult<Self> {
        Ok(Self {
            image_id: row.get(0)?,
            keyword_id: row.get(1)?,
            assigned_at: crate::db::parse_datetime(row.get::<_, String>(2)?, "assigned_at")?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Folder, Image};
    use crate::schema::initialize_schema;

    #[test]
    fn assign_keyword_to_image() {
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

        let keyword = Keyword {
            id: 0,
            keyword: "sky".into(),
        };
        let keyword_id = keyword.insert(&db).unwrap();

        let ik = ImageKeyword {
            image_id,
            keyword_id,
            assigned_at: Utc::now(),
        };
        ik.insert(&db).unwrap();

        let found = ImageKeyword::list_keywords_for_image(&db, image_id).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].keyword, "sky");
    }
}
