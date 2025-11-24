use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use anyhow::Result;
use walkdir::WalkDir;

use crate::raw::decoder::is_supported_raw_extension;

use super::CancellationFlag;

const SUPPORTED_RASTER_EXTENSIONS: &[&str] =
    &["jpg", "jpeg", "png", "tiff", "tif", "jxl", "heif", "heic"];

/// High-level classification of the asset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetType {
    JpegOnly,
    RawOnly,
    RawWithJpeg,
}

/// A candidate image to import, possibly consisting of a RAW + JPEG pair.
#[derive(Debug, Clone)]
pub struct ImportCandidate {
    pub raw_path: Option<PathBuf>,
    pub jpeg_path: Option<PathBuf>,
    pub asset_type: AssetType,
}

impl ImportCandidate {
    pub fn primary_path(&self) -> Option<&Path> {
        self.raw_path
            .as_deref()
            .or_else(|| self.jpeg_path.as_deref())
    }
}

/// Scan a folder (recursive) for images, pairing RAW+JPEG where applicable.
///
/// The walk currently includes subdirectories to match ZenithPhoto's existing import UX.
pub fn scan_folder_for_import(
    dir: &Path,
    cancel: Option<&CancellationFlag>,
) -> Result<Vec<ImportCandidate>> {
    let mut map: HashMap<(PathBuf, String), ImportCandidate> = HashMap::new();

    for entry in WalkDir::new(dir).into_iter().filter_map(|res| res.ok()) {
        if cancel
            .as_ref()
            .map(|flag| flag.is_canceled())
            .unwrap_or(false)
        {
            break;
        }

        if !entry.file_type().is_file() {
            continue;
        }

        let path = entry.into_path();
        let ext = match path.extension().and_then(OsStr::to_str) {
            Some(e) => e.to_ascii_lowercase(),
            None => continue,
        };

        let is_jpeg = matches!(ext.as_str(), "jpg" | "jpeg");
        let is_raw = is_supported_raw_extension(&format!(".{ext}"));
        let is_supported_raster = SUPPORTED_RASTER_EXTENSIONS
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(&ext));

        if !is_jpeg && !is_raw && !is_supported_raster {
            continue;
        }

        let stem = match path.file_stem().and_then(OsStr::to_str) {
            Some(s) => s.to_ascii_lowercase(),
            None => continue,
        };

        let parent = path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| dir.to_path_buf());

        let key = (parent, stem);
        let entry = map.entry(key).or_insert_with(|| ImportCandidate {
            raw_path: None,
            jpeg_path: None,
            asset_type: AssetType::JpegOnly,
        });

        if is_raw {
            entry.raw_path = Some(path.clone());
        } else {
            entry.jpeg_path = Some(path.clone());
        }
    }

    for candidate in map.values_mut() {
        match (&candidate.raw_path, &candidate.jpeg_path) {
            (Some(_), Some(_)) => candidate.asset_type = AssetType::RawWithJpeg,
            (Some(_), None) => candidate.asset_type = AssetType::RawOnly,
            _ => candidate.asset_type = AssetType::JpegOnly,
        }
    }

    let mut result: Vec<ImportCandidate> = map.into_values().collect();
    result.sort_by_key(|c| {
        c.primary_path()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(PathBuf::new)
    });

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn pairs_raw_and_jpeg_in_same_folder() {
        let dir = tempdir().unwrap();
        let raw = dir.path().join("DSC_0001.NEF");
        let jpg = dir.path().join("DSC_0001.JPG");
        fs::write(&raw, b"raw").unwrap();
        fs::write(&jpg, b"jpg").unwrap();

        let flag = CancellationFlag::default();
        let results = scan_folder_for_import(dir.path(), Some(&flag)).expect("scan");
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0].asset_type, AssetType::RawWithJpeg));
        assert_eq!(results[0].raw_path.as_ref(), Some(&raw));
        assert_eq!(results[0].jpeg_path.as_ref(), Some(&jpg));
    }

    #[test]
    fn keeps_separate_files_for_different_folders() {
        let dir = tempdir().unwrap();
        let sub_a = dir.path().join("a");
        let sub_b = dir.path().join("b");
        fs::create_dir_all(&sub_a).unwrap();
        fs::create_dir_all(&sub_b).unwrap();

        let raw_a = sub_a.join("IMG_0001.CR3");
        let raw_b = sub_b.join("IMG_0001.CR3");
        fs::write(&raw_a, b"a").unwrap();
        fs::write(&raw_b, b"b").unwrap();

        let results = scan_folder_for_import(dir.path(), None).expect("scan");
        assert_eq!(results.len(), 2);
        assert_eq!(
            results
                .iter()
                .filter(|c| matches!(c.asset_type, AssetType::RawOnly))
                .count(),
            2
        );
    }
}
