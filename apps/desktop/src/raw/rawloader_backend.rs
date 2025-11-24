use std::path::Path;

use anyhow::{anyhow, Context, Result as AnyResult};
use core_types::raw_jpeg::find_embedded_jpeg;
use image::{
    codecs::jpeg::JpegEncoder, imageops::FilterType, DynamicImage, GenericImageView, GrayImage,
};
use rawloader::{decode_file, Orientation, RawImage, RawImageData};
use tracing::debug;

use crate::raw::decoder::{RawDecoder, RawError, RawMetadata};

const THUMBNAIL_MAX_DIM: u32 = 512;
const PREVIEW_MAX_DIM: u32 = 2048;
const JPEG_QUALITY: u8 = 85;

pub struct RawloaderDecoder;

impl RawloaderDecoder {
    pub fn new() -> Self {
        Self
    }
}

impl Default for RawloaderDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl RawDecoder for RawloaderDecoder {
    fn decode_thumbnail(&self, path: &Path) -> Result<Vec<u8>, RawError> {
        debug!("Decoding RAW thumbnail via rawloader: {}", path.display());
        if let Some(bytes) = embedded_jpeg_bytes(path, THUMBNAIL_MAX_DIM).map_err(|err| {
            RawError::ThumbnailDecodeError {
                path: path.display().to_string(),
                source: err,
            }
        })? {
            return Ok(bytes);
        }

        let raw = load_raw(path)?;
        render_jpeg(&raw, THUMBNAIL_MAX_DIM).map_err(|err| RawError::ThumbnailDecodeError {
            path: path.display().to_string(),
            source: err,
        })
    }

    fn decode_preview(&self, path: &Path) -> Result<Vec<u8>, RawError> {
        debug!("Decoding RAW preview via rawloader: {}", path.display());
        if let Some(bytes) = embedded_jpeg_bytes(path, PREVIEW_MAX_DIM).map_err(|err| {
            RawError::PreviewDecodeError {
                path: path.display().to_string(),
                source: err,
            }
        })? {
            return Ok(bytes);
        }

        let raw = load_raw(path)?;
        render_jpeg(&raw, PREVIEW_MAX_DIM).map_err(|err| RawError::PreviewDecodeError {
            path: path.display().to_string(),
            source: err,
        })
    }

    fn extract_metadata(&self, path: &Path) -> Result<RawMetadata, RawError> {
        debug!("Extracting RAW metadata via rawloader: {}", path.display());
        let raw = load_raw(path)?;
        let (width, height) = dimension_components(&raw)
            .map(|(_, _, w, h)| (w, h))
            .map_err(|err| RawError::MetadataError {
                path: path.display().to_string(),
                source: err,
            })?;

        let mut meta = RawMetadata::default();
        let make = raw.make.trim();
        if !make.is_empty() {
            meta.camera_make = Some(make.to_string());
        }
        let model = raw.model.trim();
        if !model.is_empty() {
            meta.camera_model = Some(model.to_string());
        }
        meta.width = Some(width as u32);
        meta.height = Some(height as u32);
        let orientation = raw.orientation.to_u16() as i32;
        if orientation > 0 {
            meta.orientation = Some(orientation);
        }

        Ok(meta)
    }
}

fn load_raw(path: &Path) -> Result<RawImage, RawError> {
    decode_file(path).map_err(|err| RawError::OpenError {
        path: path.display().to_string(),
        source: anyhow!("rawloader error: {err}"),
    })
}

fn embedded_jpeg_bytes(path: &Path, target: u32) -> AnyResult<Option<Vec<u8>>> {
    let Some(bytes) = find_embedded_jpeg(path)
        .with_context(|| format!("failed to scan {} for embedded JPEG", path.display()))?
    else {
        return Ok(None);
    };

    let decoded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| image::load_from_memory(&bytes)));
    let image = match decoded {
        Ok(Ok(img)) => img,
        Ok(Err(err)) => return Err(err).context("embedded JPEG decode failed"),
        Err(_) => return Err(anyhow::anyhow!("embedded JPEG decoder panicked")).context("embedded JPEG decode failed"),
    };
    let resized = resize_to_fit(&image, target);
    let encoded = encode_jpeg(&resized)?;
    Ok(Some(encoded))
}

fn render_jpeg(raw: &RawImage, target: u32) -> AnyResult<Vec<u8>> {
    let dyn_img = raw_to_dynamic(raw)?;
    let oriented = apply_orientation(dyn_img, raw.orientation);
    let resized = resize_to_fit(&oriented, target);
    encode_jpeg(&resized)
}

fn raw_to_dynamic(raw: &RawImage) -> AnyResult<DynamicImage> {
    let (crop_top, crop_left, width, height) = dimension_components(raw)?;
    match &raw.data {
        RawImageData::Integer(data) => grayscale_from_integers(
            data,
            raw.width,
            raw.cpp,
            crop_top,
            crop_left,
            width,
            height,
            &raw.whitelevels,
            &raw.blacklevels,
        ),
        RawImageData::Float(data) => {
            grayscale_from_floats(data, raw.width, raw.cpp, crop_top, crop_left, width, height)
        }
    }
}

fn grayscale_from_integers(
    data: &[u16],
    full_width: usize,
    cpp: usize,
    crop_top: usize,
    crop_left: usize,
    width: usize,
    height: usize,
    whitelevels: &[u16; 4],
    blacklevels: &[u16; 4],
) -> AnyResult<DynamicImage> {
    let avg_black = blacklevels.iter().copied().map(|v| v as f32).sum::<f32>() / 4.0;
    let max_white = whitelevels.iter().copied().max().unwrap_or(65535) as f32;
    let scale = (max_white - avg_black).max(1.0);
    let cpp = cpp.max(1);

    let mut buffer = Vec::with_capacity(width * height);
    for y in 0..height {
        for x in 0..width {
            let idx = ((y + crop_top) * full_width + (x + crop_left)) * cpp;
            if idx + cpp > data.len() {
                return Err(anyhow!(
                    "Invalid RAW buffer dimensions ({}x{})",
                    width,
                    height
                ));
            }
            let mut sum = 0u32;
            for channel in 0..cpp {
                sum += data[idx + channel] as u32;
            }
            let sample = sum as f32 / cpp as f32;
            let norm = ((sample - avg_black) / scale).clamp(0.0, 1.0);
            buffer.push((norm * 255.0).round() as u8);
        }
    }

    buffer_to_dynamic(width, height, buffer)
}

fn grayscale_from_floats(
    data: &[f32],
    full_width: usize,
    cpp: usize,
    crop_top: usize,
    crop_left: usize,
    width: usize,
    height: usize,
) -> AnyResult<DynamicImage> {
    let cpp = cpp.max(1);
    let mut buffer = Vec::with_capacity(width * height);
    for y in 0..height {
        for x in 0..width {
            let idx = ((y + crop_top) * full_width + (x + crop_left)) * cpp;
            if idx + cpp > data.len() {
                return Err(anyhow!(
                    "Invalid RAW buffer dimensions ({}x{})",
                    width,
                    height
                ));
            }
            let mut sum = 0.0;
            for channel in 0..cpp {
                sum += data[idx + channel];
            }
            let sample = sum / cpp as f32;
            let norm = sample.clamp(0.0, 1.0);
            buffer.push((norm * 255.0).round() as u8);
        }
    }

    buffer_to_dynamic(width, height, buffer)
}

fn buffer_to_dynamic(width: usize, height: usize, buffer: Vec<u8>) -> AnyResult<DynamicImage> {
    let gray = GrayImage::from_vec(width as u32, height as u32, buffer)
        .context("Invalid grayscale buffer")?;
    Ok(DynamicImage::ImageLuma8(gray))
}

fn dimension_components(raw: &RawImage) -> AnyResult<(usize, usize, usize, usize)> {
    let crop_top = raw.crops.get(0).copied().unwrap_or(0).min(raw.height);
    let crop_right = raw.crops.get(1).copied().unwrap_or(0).min(raw.width);
    let crop_bottom = raw.crops.get(2).copied().unwrap_or(0).min(raw.height);
    let crop_left = raw.crops.get(3).copied().unwrap_or(0).min(raw.width);

    let width = raw
        .width
        .saturating_sub(crop_left.saturating_add(crop_right));
    let height = raw
        .height
        .saturating_sub(crop_top.saturating_add(crop_bottom));

    if width == 0 || height == 0 {
        return Err(anyhow!("Invalid RAW dimensions"));
    }

    Ok((crop_top, crop_left, width, height))
}

fn apply_orientation(image: DynamicImage, orientation: Orientation) -> DynamicImage {
    match orientation {
        Orientation::Normal | Orientation::Unknown => image,
        Orientation::HorizontalFlip => image.fliph(),
        Orientation::VerticalFlip => image.flipv(),
        Orientation::Rotate180 => image.rotate180(),
        Orientation::Rotate90 => image.rotate90(),
        Orientation::Rotate270 => image.rotate270(),
        Orientation::Transpose => image.rotate90().fliph(),
        Orientation::Transverse => image.rotate90().flipv(),
    }
}

fn resize_to_fit(image: &DynamicImage, max_dim: u32) -> DynamicImage {
    if max_dim == 0 {
        return image.clone();
    }
    let (width, height) = image.dimensions();
    if width.max(height) <= max_dim {
        image.clone()
    } else {
        image.resize(max_dim, max_dim, FilterType::Lanczos3)
    }
}

fn encode_jpeg(image: &DynamicImage) -> AnyResult<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut encoder = JpegEncoder::new_with_quality(&mut bytes, JPEG_QUALITY);
    encoder.encode_image(image).context("JPEG encode failed")?;
    Ok(bytes)
}
