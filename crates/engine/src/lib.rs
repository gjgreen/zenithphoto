use core_types::PreviewImage;
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Decode error: {0}")]
    Decode(String),
}

pub type Result<T> = std::result::Result<T, EngineError>;

pub struct ImageEngine;

impl ImageEngine {
    pub fn new() -> Self {
        Self
    }

    /// Load a file and return a preview scaled so neither dimension exceeds `max_size`.
    pub fn open_preview<P: AsRef<Path>>(&self, path: P, max_size: u32) -> Result<PreviewImage> {
        let path = path.as_ref();
        let dyn_img = image::open(path).map_err(|e| EngineError::Decode(e.to_string()))?;

        let scaled = dyn_img.thumbnail(max_size, max_size).to_rgba8();
        let (w, h) = scaled.dimensions();
        let data = scaled.into_raw();

        Ok(PreviewImage {
            width: w,
            height: h,
            data,
        })
    }
}
