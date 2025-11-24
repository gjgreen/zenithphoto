use catalog::CatalogPath;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use thiserror::Error;

const MAX_RECENT: usize = 5;

pub type Result<T> = std::result::Result<T, ConfigError>;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Invalid config data: {0}")]
    Json(#[from] serde_json::Error),
    #[cfg(not(target_os = "windows"))]
    #[error("Unable to locate configuration directory")]
    MissingConfigPath,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FolioLastSelection {
    pub kind: String,
    #[serde(default)]
    pub folder_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub recent_catalogs: Vec<PathBuf>,
    pub last_catalog: Option<PathBuf>,
    #[serde(default)]
    pub folio_last_selection: Option<FolioLastSelection>,
    #[serde(default)]
    pub last_import_timestamps: HashMap<PathBuf, String>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            recent_catalogs: Vec::new(),
            last_catalog: None,
            folio_last_selection: None,
            last_import_timestamps: HashMap::new(),
        }
    }
}

impl AppConfig {
    pub fn record_catalog(&mut self, path: impl AsRef<Path>) {
        let normalized = CatalogPath::new(path).into_path();
        self.last_catalog = Some(normalized.clone());
        self.recent_catalogs
            .retain(|existing| existing != &normalized);
        self.recent_catalogs.insert(0, normalized);
        if self.recent_catalogs.len() > MAX_RECENT {
            self.recent_catalogs.truncate(MAX_RECENT);
        }
    }
}

#[derive(Clone)]
pub struct ConfigStore {
    inner: Arc<Mutex<AppConfig>>,
}

impl ConfigStore {
    pub fn load() -> Result<Self> {
        let cfg = load_impl()?;
        Ok(Self::from_config(cfg))
    }

    pub fn new_default() -> Self {
        Self::from_config(AppConfig::default())
    }

    pub fn from_config(cfg: AppConfig) -> Self {
        Self {
            inner: Arc::new(Mutex::new(cfg)),
        }
    }

    pub fn snapshot(&self) -> AppConfig {
        self.inner.lock().expect("config poisoned").clone()
    }

    pub fn last_catalog(&self) -> Option<PathBuf> {
        self.inner
            .lock()
            .expect("config poisoned")
            .last_catalog
            .clone()
    }

    pub fn record_catalog(&self, path: impl AsRef<Path>) -> Result<AppConfig> {
        let normalized = CatalogPath::new(path).into_path();
        self.update(|cfg| {
            cfg.record_catalog(&normalized);
            true
        })
    }

    pub fn clear_recent_catalogs(&self) -> Result<AppConfig> {
        self.update(|cfg| {
            if cfg.recent_catalogs.is_empty() && cfg.last_catalog.is_none() {
                return false;
            }
            cfg.recent_catalogs.clear();
            cfg.last_catalog = None;
            true
        })
    }

    pub fn set_folio_selection(&self, selection: FolioLastSelection) -> Result<AppConfig> {
        self.update(|cfg| {
            cfg.folio_last_selection = Some(selection.clone());
            true
        })
    }

    pub fn last_folio_selection(&self) -> Option<FolioLastSelection> {
        self.inner
            .lock()
            .expect("config poisoned")
            .folio_last_selection
            .clone()
    }

    pub fn record_last_import(
        &self,
        catalog: impl AsRef<Path>,
        timestamp: &str,
    ) -> Result<AppConfig> {
        let normalized = CatalogPath::new(catalog).into_path();
        let ts = timestamp.to_string();
        self.update(|cfg| {
            cfg.last_import_timestamps
                .insert(normalized.clone(), ts.clone());
            true
        })
    }

    pub fn last_import_timestamp(&self, catalog: impl AsRef<Path>) -> Option<String> {
        let normalized = CatalogPath::new(catalog).into_path();
        self.inner
            .lock()
            .expect("config poisoned")
            .last_import_timestamps
            .get(&normalized)
            .cloned()
    }

    fn update<F>(&self, mut fun: F) -> Result<AppConfig>
    where
        F: FnMut(&mut AppConfig) -> bool,
    {
        let mut guard = self.inner.lock().expect("config poisoned");
        let changed = fun(&mut guard);
        if changed {
            save_impl(&guard)?;
        }
        Ok(guard.clone())
    }
}

#[cfg(target_os = "windows")]
fn load_impl() -> Result<AppConfig> {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ};
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = hkcu
        .open_subkey_with_flags("Software\\ZenithPhoto", KEY_READ)
        .ok();

    if let Some(key) = key {
        if let Ok(payload) = key.get_value::<String, _>("AppConfig") {
            return Ok(serde_json::from_str(&payload)?);
        }
    }

    Ok(AppConfig::default())
}

#[cfg(target_os = "windows")]
fn save_impl(cfg: &AppConfig) -> Result<()> {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_WRITE};
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _) = hkcu.create_subkey_with_flags("Software\\ZenithPhoto", KEY_WRITE)?;
    let payload = serde_json::to_string(cfg)?;
    key.set_value("AppConfig", &payload)?;
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn load_impl() -> Result<AppConfig> {
    let path = config_file_path()?;
    if path.exists() {
        let payload = std::fs::read_to_string(&path)?;
        Ok(serde_json::from_str(&payload)?)
    } else {
        Ok(AppConfig::default())
    }
}

#[cfg(not(target_os = "windows"))]
fn save_impl(cfg: &AppConfig) -> Result<()> {
    let path = config_file_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let payload = serde_json::to_string_pretty(cfg)?;
    std::fs::write(path, payload)?;
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn config_file_path() -> Result<PathBuf> {
    use directories::ProjectDirs;

    let proj_dirs = ProjectDirs::from("com", "ZenithPhoto", "ZenithPhoto")
        .ok_or(ConfigError::MissingConfigPath)?;
    let mut path = proj_dirs.config_dir().to_path_buf();
    std::fs::create_dir_all(&path)?;
    path.push("app_config.json");
    Ok(path)
}
