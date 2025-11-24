use std::path::Path;

use anyhow::{Context, Result};
use image::{codecs::jpeg::JpegEncoder, ImageReader};

/// Generate a JPEG-encoded thumbnail from a JPEG file.
/// `max_dimension` is the maximum width or height in pixels.
pub fn generate_thumbnail_from_jpeg(path: &Path, max_dimension: u32) -> Result<Vec<u8>> {
    generate_resized_jpeg(path, max_dimension, max_dimension)
}

/// Generate a JPEG-encoded preview from a JPEG file.
/// `max_long_edge` is the maximum size of the longest edge.
pub fn generate_preview_from_jpeg(path: &Path, max_long_edge: u32) -> Result<Vec<u8>> {
    generate_resized_jpeg(path, max_long_edge, max_long_edge)
}

fn generate_resized_jpeg(path: &Path, max_width: u32, max_height: u32) -> Result<Vec<u8>> {
    let reader = ImageReader::open(path).with_context(|| {
        format!(
            "Failed to open image for thumbnail/preview generation: {}",
            path.display()
        )
    })?;

    let img = reader
        .decode()
        .with_context(|| format!("Failed to decode JPEG image {}", path.display()))?;

    let resized = img.thumbnail(max_width, max_height);

    let mut buffer = Vec::new();
    let mut encoder = JpegEncoder::new_with_quality(&mut buffer, 85);
    encoder.encode_image(&resized).with_context(|| {
        format!(
            "Failed to encode resized JPEG thumbnail/preview for {}",
            path.display()
        )
    })?;

    Ok(buffer)
}
