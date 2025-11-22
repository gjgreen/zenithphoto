use crate::db::{
    from_json, parse_datetime, query_all, query_one, to_json, to_rfc3339, DbHandle, DbResult,
};
use anyhow::Context;
use chrono::{DateTime, Utc};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditHistory {
    pub id: i64,
    pub image_id: i64,
    pub edits_json: Value,
    pub created_at: DateTime<Utc>,
}

impl EditHistory {
    pub fn insert<H: DbHandle>(&self, db: &H) -> DbResult<i64> {
        let edits_json = to_json(&self.edits_json)?;
        db.execute(
            "INSERT INTO edit_history (image_id, edits_json, created_at)
             VALUES (?1, ?2, ?3)",
            params![self.image_id, edits_json, to_rfc3339(self.created_at)],
        )
        .with_context(|| {
            format!(
                "failed to insert edit_history for image_id={}",
                self.image_id
            )
        })?;
        Ok(db.last_insert_rowid())
    }

    pub fn load<H: DbHandle>(db: &H, id: i64) -> DbResult<Self> {
        query_one(
            db,
            "SELECT id, image_id, edits_json, created_at FROM edit_history WHERE id = ?1",
            params![id],
            EditHistory::from_row,
        )
        .with_context(|| format!("failed to load edit_history id={id}"))
    }

    pub fn load_all<H: DbHandle>(db: &H) -> DbResult<Vec<Self>> {
        query_all(
            db,
            "SELECT id, image_id, edits_json, created_at FROM edit_history ORDER BY created_at DESC",
            [],
            EditHistory::from_row,
        )
    }

    pub fn update<H: DbHandle>(&self, db: &H) -> DbResult<()> {
        let edits_json = to_json(&self.edits_json)?;
        db.execute(
            "UPDATE edit_history SET image_id = ?1, edits_json = ?2, created_at = ?3 WHERE id = ?4",
            params![
                self.image_id,
                edits_json,
                to_rfc3339(self.created_at),
                self.id
            ],
        )
        .with_context(|| format!("failed to update edit_history id={}", self.id))?;
        Ok(())
    }

    pub fn delete<H: DbHandle>(db: &H, id: i64) -> DbResult<()> {
        db.execute("DELETE FROM edit_history WHERE id = ?1", params![id])
            .with_context(|| format!("failed to delete edit_history id={id}"))?;
        Ok(())
    }

    fn from_row(row: &rusqlite::Row<'_>) -> DbResult<Self> {
        Ok(Self {
            id: row.get(0)?,
            image_id: row.get(1)?,
            edits_json: from_json(&row.get::<_, String>(2)?)?,
            created_at: parse_datetime(row.get::<_, String>(3)?, "created_at")?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Folder, Image};
    use crate::schema::initialize_schema;

    #[test]
    fn edit_history_serializes_json() {
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

        let history = EditHistory {
            id: 0,
            image_id,
            edits_json: serde_json::json!({"exposure": 0.1, "saturation": -0.2}),
            created_at: Utc::now(),
        };
        let hist_id = history.insert(&db).unwrap();

        let fetched = EditHistory::load(&db, hist_id).unwrap();
        assert_eq!(fetched.image_id, image_id);
        assert_eq!(fetched.edits_json["exposure"], 0.1);
    }
}
