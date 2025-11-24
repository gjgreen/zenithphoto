use std::fs;
#[cfg(target_os = "windows")]
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use anyhow::{anyhow, Context, Result};
use catalog::services::CatalogService;
use chrono::{DateTime, Utc};
use slint::{Image as SlintImage, Rgba8Pixel, SharedPixelBuffer};

mod scanner;
pub use scanner::ImportCandidate as PairedImportCandidate;

use crate::raw::decoder::RawDecoder;
use crate::raw::rsraw_backend::RsRawDecoder;
use crate::raw::thumbnail::{generate_preview_from_jpeg, generate_thumbnail_from_jpeg};

const THUMBNAIL_MAX_DIM: u32 = 256;

#[derive(Clone)]
pub struct ImportCandidate {
    pub asset: PairedImportCandidate,
    pub display_path: PathBuf,
    pub thumb: Option<SlintImage>,
}

impl ImportCandidate {
    pub fn primary_path(&self) -> Option<&Path> {
        self.asset.primary_path()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImportMethod {
    Add,
    Copy,
    Move,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DuplicateStrategy {
    Skip,
    ImportAnyway,
}

impl Default for DuplicateStrategy {
    fn default() -> Self {
        Self::Skip
    }
}

#[derive(Clone, Default)]
pub struct ImportCallbacks {
    pub progress: Option<Arc<dyn Fn(ImportProgress)>>,
    pub on_error: Option<Arc<dyn Fn(PathBuf, String)>>,
    pub duplicate_strategy: DuplicateStrategy,
    pub cancel: CancellationFlag,
}

impl ImportCallbacks {
    fn emit_progress(
        &self,
        stage: ImportStage,
        completed: usize,
        total: usize,
        message: impl Into<Option<String>>,
    ) {
        if let Some(cb) = &self.progress {
            cb(ImportProgress {
                stage,
                completed,
                total,
                message: message.into(),
            });
        }
    }

    fn emit_error(&self, path: PathBuf, err: impl Into<String>) {
        if let Some(cb) = &self.on_error {
            cb(path, err.into());
        }
    }
}

#[derive(Clone, Default)]
pub struct ScanOptions {
    pub on_candidate: Option<Arc<dyn Fn(ImportCandidate)>>,
    pub cancel: CancellationFlag,
}

#[derive(Clone, Debug, Default)]
pub struct ImportReport {
    pub imported: usize,
    pub duplicates: Vec<PathBuf>,
    pub failed: Vec<(PathBuf, String)>,
    pub canceled: bool,
    pub batch_started_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug)]
pub struct ImportProgress {
    pub stage: ImportStage,
    pub completed: usize,
    pub total: usize,
    pub message: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImportStage {
    Scanning,
    Copying,
    Moving,
    Cataloging,
    Thumbnailing,
    Keywords,
}

#[derive(Clone, Default, Debug)]
pub struct CancellationFlag(Arc<AtomicBool>);

impl CancellationFlag {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_canceled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
pub async fn scan_directory(path: &Path) -> Result<Vec<ImportCandidate>> {
    scan_directory_with_options(path, ScanOptions::default()).await
}

pub async fn scan_directory_with_options(
    path: &Path,
    options: ScanOptions,
) -> Result<Vec<ImportCandidate>> {
    scan_directory_blocking(path, options)
}

fn scan_directory_blocking(path: &Path, options: ScanOptions) -> Result<Vec<ImportCandidate>> {
    let mut out = Vec::new();
    let assets =
        scanner::scan_folder_for_import(path, Some(&options.cancel)).with_context(|| {
            format!(
                "failed to scan directory {} for import candidates",
                path.display()
            )
        })?;

    for asset in assets {
        if options.cancel.is_canceled() {
            break;
        }

        let display_path = asset
            .primary_path()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| path.to_path_buf());

        let thumb = decode_candidate_thumbnail(&asset);
        let candidate = ImportCandidate {
            asset,
            display_path,
            thumb,
        };

        if let Some(cb) = &options.on_candidate {
            cb(candidate.clone());
        }

        out.push(candidate);
    }

    Ok(out)
}

pub async fn import_images_with_callbacks(
    service: &CatalogService,
    candidates: &[PairedImportCandidate],
    keywords: &[String],
    method: ImportMethod,
    destination: Option<PathBuf>,
    callbacks: ImportCallbacks,
) -> Result<ImportReport> {
    let mut report = ImportReport::default();
    let batch_started_at = Utc::now();
    let normalized_keywords = normalize_keywords(keywords);
    let dest_dir = match method {
        ImportMethod::Add => None,
        ImportMethod::Copy | ImportMethod::Move => Some(
            destination
                .as_ref()
                .map(PathBuf::from)
                .ok_or_else(|| anyhow!("destination directory is required for copy or move"))?,
        ),
    };

    if let Some(dest_dir) = dest_dir.as_ref() {
        fs::create_dir_all(dest_dir)
            .with_context(|| format!("failed to create destination {}", dest_dir.display()))?;
    }

    let decoder = RsRawDecoder::new();
    let total = candidates.len();
    for (idx, candidate) in candidates.iter().enumerate() {
        if callbacks.cancel.is_canceled() {
            report.canceled = true;
            break;
        }

        let Some(primary_source) = candidate.primary_path() else {
            callbacks.emit_error(
                PathBuf::new(),
                "import candidate is missing file references".to_string(),
            );
            report
                .failed
                .push((PathBuf::new(), "candidate missing files".to_string()));
            continue;
        };

        let primary_source_buf = primary_source.to_path_buf();

        callbacks.emit_progress(
            ImportStage::Scanning,
            idx,
            total,
            format!("Hashing {}", primary_source.display()),
        );

        let hash = CatalogService::compute_file_hash(primary_source)
            .with_context(|| format!("failed to hash file {}", primary_source.display()))?;

        if let Some(existing) = service.find_image_by_hash(&hash)? {
            report.duplicates.push(primary_source_buf.clone());
            callbacks.emit_error(
                primary_source_buf.clone(),
                format!("duplicate detected (matches image id={})", existing.id),
            );

            if callbacks.duplicate_strategy == DuplicateStrategy::Skip {
                continue;
            }
        }

        let prepared = match prepare_candidate_paths(
            candidate,
            method,
            dest_dir.as_deref(),
            idx,
            total,
            &callbacks,
        ) {
            Ok(paths) => paths,
            Err(err) => {
                callbacks.emit_error(primary_source_buf.clone(), err.to_string());
                report
                    .failed
                    .push((primary_source_buf.clone(), err.to_string()));
                continue;
            }
        };

        let Some(target_path) = prepared.primary_path().map(Path::to_path_buf) else {
            let msg = "candidate missing files after preparation".to_string();
            callbacks.emit_error(primary_source_buf.clone(), msg.clone());
            report.failed.push((primary_source_buf.clone(), msg));
            cleanup_paths(&prepared.created_paths);
            continue;
        };

        if let Some(existing) = service.find_image_by_original_path(&target_path)? {
            report.duplicates.push(target_path.clone());
            callbacks.emit_error(
                target_path.clone(),
                format!("already imported as image id={}", existing.id),
            );
            cleanup_paths(&prepared.created_paths);
            continue;
        }

        callbacks.emit_progress(
            ImportStage::Cataloging,
            idx,
            total,
            format!("Cataloging {}", target_path.display()),
        );

        let image = match service.import_image_at(&target_path, batch_started_at) {
            Ok(img) => img,
            Err(err) => {
                callbacks.emit_error(target_path.clone(), err.to_string());
                report
                    .failed
                    .push((target_path.clone(), format!("import failed: {err}")));
                cleanup_paths(&prepared.created_paths);
                continue;
            }
        };

        if let Err(err) = service.update_sidecar_path(image.id, prepared.jpeg_path.as_deref()) {
            callbacks.emit_error(
                target_path.clone(),
                format!("failed to record JPEG sidecar: {err}"),
            );
        }

        callbacks.emit_progress(
            ImportStage::Thumbnailing,
            idx,
            total,
            format!("Generating thumbnail for {}", target_path.display()),
        );
        if let Some(jpeg_path) = prepared.jpeg_path.as_ref() {
            if let Err(err) = store_thumbnails_from_jpeg(service, image.id, jpeg_path) {
                callbacks.emit_error(jpeg_path.clone(), format!("thumbnail failed: {err}"));
            }
        } else if let Some(raw_path) = prepared.raw_path.as_ref() {
            if let Err(err) = store_thumbnails_from_raw(service, &decoder, image.id, raw_path) {
                callbacks.emit_error(raw_path.clone(), format!("raw decode failed: {err}"));
                if let Err(gen_err) = service.generate_thumbnail(image.id, raw_path) {
                    callbacks.emit_error(raw_path.clone(), format!("thumbnail failed: {gen_err}"));
                }
            }
        }

        callbacks.emit_progress(
            ImportStage::Keywords,
            idx,
            total,
            Some("Applying keywords".to_string()),
        );
        for kw in &normalized_keywords {
            if let Err(err) = service.add_keyword_to_image(image.id, kw) {
                callbacks.emit_error(target_path.clone(), format!("keyword '{kw}' failed: {err}"));
            }
        }

        if method == ImportMethod::Move {
            callbacks.emit_progress(
                ImportStage::Moving,
                idx,
                total,
                format!("Removing {}", primary_source.display()),
            );
            if let Some(raw) = candidate.raw_path.as_ref() {
                if let Err(err) = fs::remove_file(raw) {
                    callbacks.emit_error(raw.clone(), format!("failed to remove source: {err}"));
                }
            }
            if let Some(jpeg) = candidate.jpeg_path.as_ref() {
                if let Err(err) = fs::remove_file(jpeg) {
                    callbacks.emit_error(jpeg.clone(), format!("failed to remove source: {err}"));
                }
            }
        }

        report.imported += 1;
        report.batch_started_at.get_or_insert(batch_started_at);
    }

    Ok(report)
}

pub fn parse_keywords(raw: &str) -> Vec<String> {
    raw.split(',')
        .flat_map(|chunk| chunk.split('\n'))
        .map(|kw| kw.trim().to_string())
        .filter(|kw| !kw.is_empty())
        .collect()
}

/// Returns true when the file already exists in the catalog by path or hash.
pub fn is_already_imported(service: &CatalogService, path: &Path) -> bool {
    if let Ok(Some(_)) = service.find_image_by_original_path(path) {
        return true;
    }

    if let Ok(hash) = CatalogService::compute_file_hash(path) {
        if let Ok(Some(_)) = service.find_image_by_hash(&hash) {
            return true;
        }
    }

    false
}

fn normalize_keywords(keywords: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for kw in keywords {
        let trimmed = kw.trim();
        if !trimmed.is_empty() && !out.iter().any(|existing: &String| existing == trimmed) {
            out.push(trimmed.to_string());
        }
    }
    out
}

#[derive(Debug, Clone)]
struct PreparedCandidate {
    raw_path: Option<PathBuf>,
    jpeg_path: Option<PathBuf>,
    created_paths: Vec<PathBuf>,
}

impl PreparedCandidate {
    fn primary_path(&self) -> Option<&Path> {
        self.raw_path
            .as_deref()
            .or_else(|| self.jpeg_path.as_deref())
    }
}

fn copy_into_destination(src: &Path, dest_dir: &Path) -> Result<PathBuf> {
    let filename = src
        .file_name()
        .ok_or_else(|| anyhow!("source file is missing a filename: {}", src.display()))?;
    let dest_path = dest_dir.join(filename);
    fs::create_dir_all(dest_dir)
        .with_context(|| format!("failed to create destination {}", dest_dir.display()))?;
    fs::copy(src, &dest_path).with_context(|| {
        format!(
            "failed to copy {} to {}",
            src.display(),
            dest_path.display()
        )
    })?;
    Ok(dest_path)
}

fn prepare_candidate_paths(
    candidate: &PairedImportCandidate,
    method: ImportMethod,
    dest_dir: Option<&Path>,
    idx: usize,
    total: usize,
    callbacks: &ImportCallbacks,
) -> Result<PreparedCandidate> {
    let mut raw_path = candidate.raw_path.clone();
    let mut jpeg_path = candidate.jpeg_path.clone();
    let mut created_paths = Vec::new();

    if matches!(method, ImportMethod::Copy | ImportMethod::Move) {
        let destination = dest_dir.expect("validated destination directory");
        if let Some(raw) = candidate.raw_path.as_ref() {
            callbacks.emit_progress(
                ImportStage::Copying,
                idx,
                total,
                format!("Copying {}", raw.display()),
            );
            let copied = copy_into_destination(raw, destination)?;
            created_paths.push(copied.clone());
            raw_path = Some(copied);
        }
        if let Some(jpeg) = candidate.jpeg_path.as_ref() {
            callbacks.emit_progress(
                ImportStage::Copying,
                idx,
                total,
                format!("Copying {}", jpeg.display()),
            );
            let copied = copy_into_destination(jpeg, destination)?;
            created_paths.push(copied.clone());
            jpeg_path = Some(copied);
        }
    }

    Ok(PreparedCandidate {
        raw_path,
        jpeg_path,
        created_paths,
    })
}

fn cleanup_paths(paths: &[PathBuf]) {
    for path in paths {
        if let Err(err) = fs::remove_file(path) {
            eprintln!("Failed to remove {}: {err}", path.display());
        }
    }
}

fn store_thumbnails_from_jpeg(
    service: &CatalogService,
    image_id: i64,
    jpeg_path: &Path,
) -> Result<()> {
    let thumb_small = generate_thumbnail_from_jpeg(jpeg_path, 256)?;
    let thumb_large = generate_thumbnail_from_jpeg(jpeg_path, 1024)?;
    service
        .upsert_thumbnail(image_id, Some(thumb_small), Some(thumb_large))
        .context("failed to store JPEG thumbnails")?;

    let preview = generate_preview_from_jpeg(jpeg_path, 2048)?;
    service
        .upsert_preview_placeholder(image_id, Some(preview))
        .context("failed to store JPEG preview")?;
    Ok(())
}

fn store_thumbnails_from_raw(
    service: &CatalogService,
    decoder: &dyn RawDecoder,
    image_id: i64,
    raw_path: &Path,
) -> Result<()> {
    let thumb = decoder
        .decode_thumbnail(raw_path)
        .map_err(|err| anyhow!(err))?;
    let preview = decoder
        .decode_preview(raw_path)
        .map_err(|err| anyhow!(err))?;
    service
        .upsert_thumbnail(image_id, Some(thumb), None)
        .context("failed to store RAW thumbnails")?;
    service
        .upsert_preview_placeholder(image_id, Some(preview))
        .context("failed to store RAW preview")?;
    Ok(())
}

fn decode_candidate_thumbnail(asset: &PairedImportCandidate) -> Option<SlintImage> {
    if let Some(jpeg_path) = asset.jpeg_path.as_deref() {
        if let Some(img) = decode_thumbnail_from_path(jpeg_path) {
            return Some(img);
        }
    }

    if let Some(raw_path) = asset.raw_path.as_deref() {
        if let Some(img) = decode_thumbnail_from_raw(raw_path) {
            return Some(img);
        }
        if let Some(img) = decode_thumbnail_from_path(raw_path) {
            return Some(img);
        }
        if let Some(img) = decode_thumbnail_with_shell(raw_path) {
            return Some(img);
        }
    }

    if let Some(primary) = asset.primary_path() {
        if let Some(img) = decode_thumbnail_with_shell(primary) {
            return Some(img);
        }
    }

    None
}

fn decode_thumbnail_from_raw(path: &Path) -> Option<SlintImage> {
    let decoder = RsRawDecoder::new();
    let bytes = decoder.decode_thumbnail(path).ok()?;
    decode_thumbnail_from_bytes(&bytes)
}

fn decode_thumbnail_from_path(path: &Path) -> Option<SlintImage> {
    let dyn_img = image::open(path).ok()?;
    convert_dynamic_to_slint(&dyn_img)
}

fn decode_thumbnail_from_bytes(bytes: &[u8]) -> Option<SlintImage> {
    let dyn_img = load_image_from_memory_safe(bytes)?;
    convert_dynamic_to_slint(&dyn_img)
}

fn load_image_from_memory_safe(bytes: &[u8]) -> Option<image::DynamicImage> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| image::load_from_memory(bytes)))
    {
        Ok(Ok(img)) => Some(img),
        Ok(Err(err)) => {
            eprintln!("Failed to decode thumbnail bytes: {err}");
            None
        }
        Err(_) => {
            eprintln!("Thumbnail decoder panicked");
            None
        }
    }
}

fn convert_dynamic_to_slint(dyn_img: &image::DynamicImage) -> Option<SlintImage> {
    let thumb = letterbox_thumbnail(&dyn_img, THUMBNAIL_MAX_DIM);
    let (w, h) = thumb.dimensions();
    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(w, h);
    buf.make_mut_bytes().copy_from_slice(thumb.as_raw());
    Some(SlintImage::from_rgba8(buf))
}

#[cfg(target_os = "windows")]
fn decode_thumbnail_with_shell(path: &Path) -> Option<SlintImage> {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{RPC_E_CHANGED_MODE, SIZE};
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits, GetObjectW, BITMAP, BITMAPINFO,
        BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP,
    };
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
    use windows::Win32::UI::Shell::{
        IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_BIGGERSIZEOK,
        SIIGBF_RESIZETOFIT,
    };

    unsafe {
        let hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let need_uninit = if hr.is_ok() {
            true
        } else if hr == RPC_E_CHANGED_MODE {
            false
        } else {
            return None;
        };

        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let factory: IShellItemImageFactory =
            SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None).ok()?;

        let hbitmap: HBITMAP = factory
            .GetImage(
                SIZE {
                    cx: THUMBNAIL_MAX_DIM as i32,
                    cy: THUMBNAIL_MAX_DIM as i32,
                },
                SIIGBF_RESIZETOFIT | SIIGBF_BIGGERSIZEOK,
            )
            .ok()?;

        let hdc = CreateCompatibleDC(None);
        if hdc.0.is_null() {
            let _ = DeleteObject(hbitmap.into());
            if need_uninit {
                CoUninitialize();
            }
            return None;
        }

        let mut bmp = BITMAP::default();
        if GetObjectW(
            hbitmap.into(),
            std::mem::size_of::<BITMAP>() as i32,
            Some(&mut bmp as *mut _ as *mut _),
        ) == 0
        {
            let _ = DeleteDC(hdc);
            let _ = DeleteObject(hbitmap.into());
            if need_uninit {
                CoUninitialize();
            }
            return None;
        }
        let width = bmp.bmWidth;
        let height = bmp.bmHeight;
        if width <= 0 || height <= 0 {
            let _ = DeleteDC(hdc);
            let _ = DeleteObject(hbitmap.into());
            if need_uninit {
                CoUninitialize();
            }
            return None;
        }

        let mut info = BITMAPINFO::default();
        info.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        };

        let mut pixels = vec![0u8; (width * height * 4) as usize];
        if GetDIBits(
            hdc,
            hbitmap,
            0,
            height as u32,
            Some(pixels.as_mut_ptr() as *mut _),
            &mut info,
            DIB_RGB_COLORS,
        ) == 0
        {
            let _ = DeleteDC(hdc);
            let _ = DeleteObject(hbitmap.into());
            if need_uninit {
                CoUninitialize();
            }
            return None;
        }

        let _ = DeleteDC(hdc);
        let _ = DeleteObject(hbitmap.into());
        if need_uninit {
            CoUninitialize();
        }

        for chunk in pixels.chunks_exact_mut(4) {
            chunk.swap(0, 2);
        }

        let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(width as u32, height as u32);
        buf.make_mut_bytes().copy_from_slice(&pixels);
        Some(SlintImage::from_rgba8(buf))
    }
}

#[cfg(not(target_os = "windows"))]
fn decode_thumbnail_with_shell(_path: &Path) -> Option<SlintImage> {
    None
}

fn letterbox_thumbnail(img: &image::DynamicImage, max_dim: u32) -> image::RgbaImage {
    let resized = img
        .resize(max_dim, max_dim, image::imageops::FilterType::Lanczos3)
        .to_rgba8();
    let (w, h) = resized.dimensions();
    if w == max_dim && h == max_dim {
        return resized;
    }

    let mut canvas = image::RgbaImage::from_pixel(max_dim, max_dim, image::Rgba([16, 16, 16, 255]));
    let offset_x = (max_dim - w) / 2;
    let offset_y = (max_dim - h) / 2;
    image::imageops::overlay(&mut canvas, &resized, offset_x.into(), offset_y.into());
    canvas
}

#[cfg(test)]
mod tests {
    use super::scanner::AssetType;
    use super::*;
    use catalog::db::CatalogDb;
    use catalog::schema::initialize_schema;
    use futures::executor::block_on;
    use tempfile::tempdir;

    fn write_test_image(path: &Path) {
        let seed = (path.to_string_lossy().len() as u8)
            .wrapping_mul(17)
            .wrapping_add(3);
        let img = image::RgbaImage::from_pixel(
            64,
            64,
            image::Rgba([seed, seed.wrapping_add(10), 200, 255]),
        );
        img.save(path).unwrap();
    }

    fn single_file_candidate(path: &Path) -> PairedImportCandidate {
        PairedImportCandidate {
            raw_path: None,
            jpeg_path: Some(path.to_path_buf()),
            asset_type: AssetType::JpegOnly,
        }
    }

    fn service_with_memory_db() -> CatalogService {
        let db = CatalogDb::in_memory().unwrap();
        initialize_schema(db.conn()).unwrap();
        CatalogService::new(db)
    }

    #[test]
    fn scan_directory_filters_supported_extensions() {
        let dir = tempdir().unwrap();
        let jpg_path = dir.path().join("one.JPG");
        let txt_path = dir.path().join("note.txt");
        write_test_image(&jpg_path);
        fs::write(&txt_path, b"ignore me").unwrap();

        let results = block_on(scan_directory(dir.path())).expect("scan");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].display_path, jpg_path);
        assert!(results[0].thumb.is_some());
    }

    #[test]
    fn import_add_and_move_workflow() {
        let service = service_with_memory_db();
        let dir = tempdir().unwrap();

        let src_add = dir.path().join("photo.png");
        write_test_image(&src_add);

        let report = block_on(import_images_with_callbacks(
            &service,
            &[single_file_candidate(&src_add)],
            &[" summer ".to_string(), "summer".to_string()],
            ImportMethod::Add,
            None,
            ImportCallbacks::default(),
        ))
        .expect("add import");
        assert_eq!(report.imported, 1);
        assert!(report.duplicates.is_empty());
        assert_eq!(service.count_images().unwrap(), 1);

        // Move into a new folder
        let src_move = dir.path().join("photo_move.png");
        write_test_image(&src_move);
        let dest_dir = dir.path().join("moved");
        let report = block_on(import_images_with_callbacks(
            &service,
            &[single_file_candidate(&src_move)],
            &[],
            ImportMethod::Move,
            Some(dest_dir.clone()),
            ImportCallbacks::default(),
        ))
        .expect("move import");
        assert_eq!(report.imported, 1);
        assert!(dest_dir.join("photo_move.png").exists());
        assert!(!src_move.exists());
        assert_eq!(service.count_images().unwrap(), 2);
    }
}
