use crate::db::{
    from_json, parse_datetime_opt, query_all, query_one, to_json, to_rfc3339_opt, DbHandle,
    DbResult,
};
use anyhow::Context;
use chrono::{DateTime, Utc};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edit {
    pub id: i64,
    pub image_id: i64,
    pub exposure: Option<f64>,
    pub contrast: Option<f64>,
    pub highlights: Option<f64>,
    pub shadows: Option<f64>,
    pub whites: Option<f64>,
    pub blacks: Option<f64>,
    pub vibrance: Option<f64>,
    pub saturation: Option<f64>,
    pub temperature: Option<f64>,
    pub tint: Option<f64>,
    pub texture: Option<f64>,
    pub clarity: Option<f64>,
    pub dehaze: Option<f64>,
    pub parametric_curve_json: Option<Value>,
    pub color_grading_json: Option<Value>,
    pub crop_json: Option<Value>,
    pub masking_json: Option<Value>,
    pub updated_at: Option<DateTime<Utc>>,
}

impl Edit {
    pub fn insert<H: DbHandle>(&self, db: &H) -> DbResult<i64> {
        let parametric_curve_json = self
            .parametric_curve_json
            .as_ref()
            .map(to_json)
            .transpose()?;
        let color_grading_json = self.color_grading_json.as_ref().map(to_json).transpose()?;
        let crop_json = self.crop_json.as_ref().map(to_json).transpose()?;
        let masking_json = self.masking_json.as_ref().map(to_json).transpose()?;
        db.execute(
            "INSERT INTO edits (
                image_id, exposure, contrast, highlights, shadows, whites, blacks,
                vibrance, saturation, temperature, tint, texture, clarity, dehaze,
                parametric_curve_json, color_grading_json, crop_json, masking_json, updated_at
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7,
                ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                ?15, ?16, ?17, ?18, ?19
            )",
            params![
                self.image_id,
                self.exposure,
                self.contrast,
                self.highlights,
                self.shadows,
                self.whites,
                self.blacks,
                self.vibrance,
                self.saturation,
                self.temperature,
                self.tint,
                self.texture,
                self.clarity,
                self.dehaze,
                parametric_curve_json,
                color_grading_json,
                crop_json,
                masking_json,
                to_rfc3339_opt(self.updated_at)
            ],
        )
        .with_context(|| format!("failed to insert edits for image_id={}", self.image_id))?;
        Ok(db.last_insert_rowid())
    }

    pub fn load<H: DbHandle>(db: &H, id: i64) -> DbResult<Self> {
        query_one(
            db,
            "SELECT
                id, image_id, exposure, contrast, highlights, shadows, whites, blacks,
                vibrance, saturation, temperature, tint, texture, clarity, dehaze,
                parametric_curve_json, color_grading_json, crop_json, masking_json, updated_at
             FROM edits WHERE id = ?1",
            params![id],
            Edit::from_row,
        )
        .with_context(|| format!("failed to load edits id={id}"))
    }

    pub fn load_all<H: DbHandle>(db: &H) -> DbResult<Vec<Self>> {
        query_all(
            db,
            "SELECT
                id, image_id, exposure, contrast, highlights, shadows, whites, blacks,
                vibrance, saturation, temperature, tint, texture, clarity, dehaze,
                parametric_curve_json, color_grading_json, crop_json, masking_json, updated_at
             FROM edits ORDER BY id",
            [],
            Edit::from_row,
        )
    }

    pub fn update<H: DbHandle>(&self, db: &H) -> DbResult<()> {
        let parametric_curve_json = self
            .parametric_curve_json
            .as_ref()
            .map(to_json)
            .transpose()?;
        let color_grading_json = self.color_grading_json.as_ref().map(to_json).transpose()?;
        let crop_json = self.crop_json.as_ref().map(to_json).transpose()?;
        let masking_json = self.masking_json.as_ref().map(to_json).transpose()?;
        db.execute(
            "UPDATE edits SET
                image_id = ?1,
                exposure = ?2,
                contrast = ?3,
                highlights = ?4,
                shadows = ?5,
                whites = ?6,
                blacks = ?7,
                vibrance = ?8,
                saturation = ?9,
                temperature = ?10,
                tint = ?11,
                texture = ?12,
                clarity = ?13,
                dehaze = ?14,
                parametric_curve_json = ?15,
                color_grading_json = ?16,
                crop_json = ?17,
                masking_json = ?18,
                updated_at = ?19
             WHERE id = ?20",
            params![
                self.image_id,
                self.exposure,
                self.contrast,
                self.highlights,
                self.shadows,
                self.whites,
                self.blacks,
                self.vibrance,
                self.saturation,
                self.temperature,
                self.tint,
                self.texture,
                self.clarity,
                self.dehaze,
                parametric_curve_json,
                color_grading_json,
                crop_json,
                masking_json,
                to_rfc3339_opt(self.updated_at),
                self.id
            ],
        )
        .with_context(|| format!("failed to update edits id={}", self.id))?;
        Ok(())
    }

    pub fn delete<H: DbHandle>(db: &H, id: i64) -> DbResult<()> {
        db.execute("DELETE FROM edits WHERE id = ?1", params![id])
            .with_context(|| format!("failed to delete edits id={id}"))?;
        Ok(())
    }

    fn from_row(row: &rusqlite::Row<'_>) -> DbResult<Self> {
        Ok(Self {
            id: row.get(0)?,
            image_id: row.get(1)?,
            exposure: row.get(2)?,
            contrast: row.get(3)?,
            highlights: row.get(4)?,
            shadows: row.get(5)?,
            whites: row.get(6)?,
            blacks: row.get(7)?,
            vibrance: row.get(8)?,
            saturation: row.get(9)?,
            temperature: row.get(10)?,
            tint: row.get(11)?,
            texture: row.get(12)?,
            clarity: row.get(13)?,
            dehaze: row.get(14)?,
            parametric_curve_json: {
                let raw: Option<String> = row.get(15)?;
                match raw {
                    Some(value) => Some(from_json(&value)?),
                    None => None,
                }
            },
            color_grading_json: {
                let raw: Option<String> = row.get(16)?;
                match raw {
                    Some(value) => Some(from_json(&value)?),
                    None => None,
                }
            },
            crop_json: {
                let raw: Option<String> = row.get(17)?;
                match raw {
                    Some(value) => Some(from_json(&value)?),
                    None => None,
                }
            },
            masking_json: {
                let raw: Option<String> = row.get(18)?;
                match raw {
                    Some(value) => Some(from_json(&value)?),
                    None => None,
                }
            },
            updated_at: parse_datetime_opt(row.get::<_, Option<String>>(19)?, "updated_at")?,
        })
    }
}
