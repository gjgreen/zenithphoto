use std::path::Path;

use anyhow::Result;

/// List of supported RAW extensions (lowercase, including the dot).
pub const SUPPORTED_RAW_EXTENSIONS: &[&str] = &[
    ".raf", // Fujifilm
    ".cr2", ".cr3", // Canon
    ".nef", // Nikon
    ".arw", // Sony
    ".orf", // Olympus
    ".dng", // DNG
];

/// Error type for RAW decoding and metadata extraction.
#[allow(dead_code)]
#[derive(Debug, thiserror::Error)]
pub enum RawError {
    #[error("RAW backend not available: {0}")]
    BackendUnavailable(String),

    #[error("Failed to open RAW file {path}: {source}")]
    OpenError {
        path: String,
        #[source]
        source: anyhow::Error,
    },

    #[error("Failed to decode RAW thumbnail for {path}: {source}")]
    ThumbnailDecodeError {
        path: String,
        #[source]
        source: anyhow::Error,
    },

    #[error("Failed to decode RAW preview for {path}: {source}")]
    PreviewDecodeError {
        path: String,
        #[source]
        source: anyhow::Error,
    },

    #[error("Failed to extract RAW metadata for {path}: {source}")]
    MetadataError {
        path: String,
        #[source]
        source: anyhow::Error,
    },
}

/// RAW-specific metadata extracted from the file.
///
/// Note: for simplicity, dates are stored as ISO-8601 strings.
/// The calling code can parse into chrono types if desired.
#[allow(dead_code)]
#[derive(Debug, Clone, Default)]
pub struct RawMetadata {
    pub capture_date: Option<String>,
    pub camera_make: Option<String>,
    pub camera_model: Option<String>,
    pub lens_model: Option<String>,
    pub focal_length: Option<f32>,
    pub shutter_speed: Option<f32>,
    pub aperture: Option<f32>,
    pub iso: Option<u32>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub orientation: Option<i32>,
}

/// Metadata + derived assets ready for catalog insertion.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct ExtractedMetadata {
    pub raw_path: Option<String>,
    pub jpeg_path: Option<String>,
    pub has_raw: bool,
    pub thumbnail: Vec<u8>,
    pub preview: Vec<u8>,
    pub capture_date: Option<String>,
    pub camera_make: Option<String>,
    pub camera_model: Option<String>,
    pub lens_model: Option<String>,
    pub focal_length: Option<f32>,
    pub shutter_speed: Option<f32>,
    pub aperture: Option<f32>,
    pub iso: Option<u32>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub orientation: Option<i32>,
}

#[allow(dead_code)]
impl ExtractedMetadata {
    pub fn from_raw(
        raw_path: Option<String>,
        jpeg_path: Option<String>,
        has_raw: bool,
        raw_meta: RawMetadata,
        thumbnail: Vec<u8>,
        preview: Vec<u8>,
    ) -> Self {
        Self {
            raw_path,
            jpeg_path,
            has_raw,
            thumbnail,
            preview,
            capture_date: raw_meta.capture_date,
            camera_make: raw_meta.camera_make,
            camera_model: raw_meta.camera_model,
            lens_model: raw_meta.lens_model,
            focal_length: raw_meta.focal_length,
            shutter_speed: raw_meta.shutter_speed,
            aperture: raw_meta.aperture,
            iso: raw_meta.iso,
            width: raw_meta.width,
            height: raw_meta.height,
            orientation: raw_meta.orientation,
        }
    }
}

/// Trait for RAW decoders. Backend implementations (e.g. LibRaw) must implement this.
pub trait RawDecoder: Send + Sync + 'static {
    /// Decode a small embedded thumbnail (for grid view).
    fn decode_thumbnail(&self, path: &Path) -> Result<Vec<u8>, RawError>;

    /// Decode a medium-resolution preview (~2k on long edge) for the Develop module.
    fn decode_preview(&self, path: &Path) -> Result<Vec<u8>, RawError>;

    /// Extract EXIF/metadata from the RAW file.
    #[allow(dead_code)]
    fn extract_metadata(&self, path: &Path) -> Result<RawMetadata, RawError>;
}

/// Quick check for whether a file extension is a supported RAW format.
pub fn is_supported_raw_extension(ext: &str) -> bool {
    let ext_lower = ext.to_ascii_lowercase();
    SUPPORTED_RAW_EXTENSIONS.contains(&ext_lower.as_str())
}
