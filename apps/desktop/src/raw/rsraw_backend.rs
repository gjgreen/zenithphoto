use std::fs;
use std::path::Path;

use anyhow::{anyhow, Context};
use rsraw::{RawImage, ThumbFormat};
use tracing::{debug, warn};

use crate::raw::decoder::{RawDecoder, RawError, RawMetadata};

/// RAW decoder built on top of the `rsraw` crate (LibRaw bindings).
///
/// This implementation focuses on the highest quality embedded thumbnail /
/// preview for now. When the Refine module lands we can extend it to call
/// `RawImage::process::<rsraw::BIT_DEPTH_16>()` for full-resolution data.
pub struct RsRawDecoder;

impl RsRawDecoder {
    pub fn new() -> Self {
        Self
    }
}

impl Default for RsRawDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl RawDecoder for RsRawDecoder {
    fn decode_thumbnail(&self, path: &Path) -> Result<Vec<u8>, RawError> {
        debug!("RsRawDecoder::decode_thumbnail {}", path.display());
        decode_embedded_thumbnail(path, RawTask::Thumbnail)
    }

    fn decode_preview(&self, path: &Path) -> Result<Vec<u8>, RawError> {
        debug!("RsRawDecoder::decode_preview {}", path.display());
        decode_embedded_thumbnail(path, RawTask::Preview)
    }

    fn extract_metadata(&self, path: &Path) -> Result<RawMetadata, RawError> {
        debug!("RsRawDecoder::extract_metadata {}", path.display());
        let task = RawTask::Metadata;
        let data = fs::read(path)
            .with_context(|| format!("Failed to read RAW file for {}", task.description()))
            .map_err(|err| task.error(path, err))?;

        let raw = RawImage::open(&data)
            .map_err(|err| task.error(path, anyhow!("rsraw::RawImage::open failed: {err:?}")))?;

        let info = raw.full_info();
        let mut meta = RawMetadata::default();

        meta.capture_date = info
            .datetime
            .map(|dt| dt.format("%Y:%m:%d %H:%M:%S").to_string());

        let make = info.make.trim();
        if !make.is_empty() {
            meta.camera_make = Some(make.to_string());
        }

        let model = info.model.trim();
        if !model.is_empty() {
            meta.camera_model = Some(model.to_string());
        }

        let lens_name = info.lens_info.lens_name.trim();
        if !lens_name.is_empty() {
            meta.lens_model = Some(lens_name.to_string());
        } else {
            let lens_make = info.lens_info.lens_make.trim();
            if !lens_make.is_empty() {
                meta.lens_model = Some(lens_make.to_string());
            }
        }

        if info.focal_len.is_finite() && info.focal_len > 0.0 {
            meta.focal_length = Some(info.focal_len);
        }
        if info.shutter.is_finite() && info.shutter > 0.0 {
            meta.shutter_speed = Some(info.shutter);
        }
        if info.aperture.is_finite() && info.aperture > 0.0 {
            meta.aperture = Some(info.aperture);
        }
        if info.iso_speed > 0 {
            meta.iso = Some(info.iso_speed);
        }

        if info.width > 0 {
            meta.width = Some(info.width);
        }
        if info.height > 0 {
            meta.height = Some(info.height);
        }

        let orientation = raw.as_ref().sizes.flip as i32;
        if orientation > 0 {
            meta.orientation = Some(orientation);
        }

        Ok(meta)
    }
}

fn decode_embedded_thumbnail(path: &Path, task: RawTask) -> Result<Vec<u8>, RawError> {
    let data = fs::read(path)
        .with_context(|| format!("Failed to read RAW file for {}", task.description()))
        .map_err(|err| task.error(path, err))?;

    let mut raw = RawImage::open(&data)
        .map_err(|err| task.error(path, anyhow!("rsraw::RawImage::open failed: {err:?}")))?;

    let thumbs = raw
        .extract_thumbs()
        .map_err(|err| task.error(path, anyhow!("rsraw::RawImage::extract_thumbs failed: {err:?}")))?;

    if thumbs.is_empty() {
        return Err(task.error(
            path,
            anyhow!("No embedded thumbnails in RAW file"),
        ));
    }

    let best = thumbs
        .into_iter()
        .max_by_key(|thumb| (thumb.width as u64) * (thumb.height as u64))
        .expect("vector not empty");

    if !matches!(best.format, ThumbFormat::Jpeg) {
        warn!(
            "Largest embedded thumbnail for {} is {:?}; returning raw bytes",
            path.display(),
            best.format
        );
    }

    if best.data.is_empty() {
        return Err(task.error(
            path,
            anyhow!("Embedded thumbnail contained no data"),
        ));
    }

    Ok(best.data)
}

#[derive(Copy, Clone)]
enum RawTask {
    Thumbnail,
    Preview,
    Metadata,
}

impl RawTask {
    fn description(self) -> &'static str {
        match self {
            RawTask::Thumbnail => "thumbnail decoding",
            RawTask::Preview => "preview decoding",
            RawTask::Metadata => "metadata extraction",
        }
    }

    fn error(self, path: &Path, source: anyhow::Error) -> RawError {
        let path = path.display().to_string();
        match self {
            RawTask::Thumbnail => RawError::ThumbnailDecodeError { path, source },
            RawTask::Preview => RawError::PreviewDecodeError { path, source },
            RawTask::Metadata => RawError::MetadataError { path, source },
        }
    }
}
