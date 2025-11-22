use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use anyhow::{anyhow, Context, Result};
use catalog::services::CatalogService;
use slint::{Image as SlintImage, Rgba8Pixel, SharedPixelBuffer};
use walkdir::WalkDir;

const SUPPORTED_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "png", "tiff", "tif", "jxl", "heif", "heic", "dng", "cr2", "nef", "raf", "arw",
];

#[derive(Clone)]
pub struct ImportCandidate {
    pub path: PathBuf,
    pub extension: String,
    pub thumb: Option<SlintImage>,
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
    for entry in WalkDir::new(path).into_iter().filter_map(|e| e.ok()) {
        if options.cancel.is_canceled() {
            break;
        }

        if !entry.file_type().is_file() {
            continue;
        }

        let ext = match entry
            .path()
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| s.to_ascii_lowercase())
        {
            Some(ext) if is_supported_extension(&ext) => ext,
            _ => continue,
        };

        let thumb = decode_thumbnail(entry.path());
        let candidate = ImportCandidate {
            path: entry.path().to_path_buf(),
            extension: ext,
            thumb,
        };

        if let Some(cb) = &options.on_candidate {
            cb(candidate.clone());
        }

        out.push(candidate);
    }

    Ok(out)
}

pub async fn import_images(
    service: &CatalogService,
    file_paths: &[PathBuf],
    keywords: &[String],
    method: ImportMethod,
    destination: Option<PathBuf>,
) -> Result<()> {
    import_images_with_callbacks(
        service,
        file_paths,
        keywords,
        method,
        destination,
        ImportCallbacks::default(),
    )
    .await
    .map(|_| ())
}

pub async fn import_images_with_callbacks(
    service: &CatalogService,
    file_paths: &[PathBuf],
    keywords: &[String],
    method: ImportMethod,
    destination: Option<PathBuf>,
    callbacks: ImportCallbacks,
) -> Result<ImportReport> {
    let mut report = ImportReport::default();
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

    let total = file_paths.len();
    for (idx, src) in file_paths.iter().enumerate() {
        if callbacks.cancel.is_canceled() {
            report.canceled = true;
            break;
        }

        let hash = CatalogService::compute_file_hash(src)
            .with_context(|| format!("failed to hash file {}", src.display()))?;

        if let Some(existing) = service.find_image_by_hash(&hash)? {
            report.duplicates.push(src.clone());
            callbacks.emit_error(
                src.clone(),
                format!("duplicate detected (matches image id={})", existing.id),
            );

            if callbacks.duplicate_strategy == DuplicateStrategy::Skip {
                continue;
            }
        }

        let target_path = match method {
            ImportMethod::Add => src.to_path_buf(),
            ImportMethod::Copy | ImportMethod::Move => {
                let dest_dir = dest_dir.as_ref().expect("validated destination");
                callbacks.emit_progress(
                    ImportStage::Copying,
                    idx,
                    total,
                    format!("Copying {}", src.display()),
                );
                copy_into_destination(src, dest_dir)?
            }
        };

        if let Some(existing) = service.find_image_by_original_path(&target_path)? {
            report.duplicates.push(target_path.clone());
            callbacks.emit_error(
                target_path.clone(),
                format!("already imported as image id={}", existing.id),
            );
            continue;
        }

        callbacks.emit_progress(
            ImportStage::Cataloging,
            idx,
            total,
            format!("Cataloging {}", target_path.display()),
        );

        let image = match service.import_image(&target_path) {
            Ok(img) => img,
            Err(err) => {
                callbacks.emit_error(target_path.clone(), err.to_string());
                report
                    .failed
                    .push((target_path.clone(), format!("import failed: {err}")));
                if matches!(method, ImportMethod::Copy | ImportMethod::Move) {
                    let _ = fs::remove_file(&target_path);
                }
                continue;
            }
        };

        callbacks.emit_progress(
            ImportStage::Thumbnailing,
            idx,
            total,
            format!("Generating thumbnail for {}", target_path.display()),
        );
        if let Err(err) = service.generate_thumbnail(image.id, &target_path) {
            callbacks.emit_error(target_path.clone(), format!("thumbnail failed: {err}"));
        }

        callbacks.emit_progress(
            ImportStage::Keywords,
            idx,
            total,
            Some("Applying keywords".to_string()),
        );
        for kw in &normalized_keywords {
            if let Err(err) = service.add_keyword_to_image(image.id, kw) {
                callbacks.emit_error(
                    target_path.clone(),
                    format!("keyword '{kw}' failed: {err}"),
                );
            }
        }

        if method == ImportMethod::Move {
            callbacks.emit_progress(
                ImportStage::Moving,
                idx,
                total,
                format!("Removing {}", src.display()),
            );
            if let Err(err) = fs::remove_file(src) {
                callbacks.emit_error(src.clone(), format!("failed to remove source: {err}"));
            }
        }

        report.imported += 1;
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

fn copy_into_destination(src: &Path, dest_dir: &Path) -> Result<PathBuf> {
    let filename = src
        .file_name()
        .ok_or_else(|| anyhow!("source file is missing a filename: {}", src.display()))?;
    let dest_path = dest_dir.join(filename);
    fs::create_dir_all(dest_dir)
        .with_context(|| format!("failed to create destination {}", dest_dir.display()))?;
    fs::copy(src, &dest_path)
        .with_context(|| format!("failed to copy {} to {}", src.display(), dest_path.display()))?;
    Ok(dest_path)
}

fn decode_thumbnail(path: &Path) -> Option<SlintImage> {
    let dyn_img = image::open(path).ok()?;
    let thumb = dyn_img.thumbnail(256, 256).to_rgba8();
    let (w, h) = thumb.dimensions();
    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(w, h);
    buf.make_mut_bytes().copy_from_slice(&thumb);
    Some(SlintImage::from_rgba8(buf))
}

fn is_supported_extension(ext: &str) -> bool {
    SUPPORTED_EXTENSIONS
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(ext))
}

#[cfg(test)]
mod tests {
    use super::*;
    use catalog::db::CatalogDb;
    use catalog::schema::initialize_schema;
    use futures::executor::block_on;
    use tempfile::tempdir;

    fn write_test_image(path: &Path) {
        let img = image::RgbaImage::from_pixel(64, 64, image::Rgba([120, 10, 200, 255]));
        img.save(path).unwrap();
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
        assert_eq!(results[0].path, jpg_path);
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
            &[src_add.clone()],
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
            &[src_move.clone()],
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
