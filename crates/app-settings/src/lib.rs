use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppSettingsError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Settings parse error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Settings path unavailable")]
    MissingSettingsPath,
}

pub type Result<T> = std::result::Result<T, AppSettingsError>;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppSettings {
    pub last_catalog: Option<PathBuf>,
}

impl AppSettings {
    pub fn load() -> Result<Self> {
        load_impl()
    }

    pub fn save(&self) -> Result<()> {
        save_impl(self)
    }

    pub fn get_last_catalog(&self) -> Option<PathBuf> {
        self.last_catalog.clone()
    }

    pub fn set_last_catalog(&mut self, path: PathBuf) {
        self.last_catalog = Some(path);
    }
}

#[cfg(target_os = "windows")]
fn load_impl() -> Result<AppSettings> {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ};
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = hkcu
        .open_subkey_with_flags("Software\\ZenithPhoto", KEY_READ)
        .ok();

    if let Some(key) = key {
        if let Ok(path) = key.get_value::<String, _>("LastCatalog") {
            return Ok(AppSettings {
                last_catalog: Some(PathBuf::from(path)),
            });
        }
    }

    Ok(AppSettings::default())
}

#[cfg(target_os = "windows")]
fn save_impl(settings: &AppSettings) -> Result<()> {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_WRITE};
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _) = hkcu.create_subkey_with_flags("Software\\ZenithPhoto", KEY_WRITE)?;

    if let Some(path) = &settings.last_catalog {
        let value = path.to_string_lossy();
        key.set_value("LastCatalog", &value.as_ref())?;
    } else {
        let _ = key.delete_value("LastCatalog");
    }

    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn load_impl() -> Result<AppSettings> {
    let path = settings_file_path()?;
    if path.exists() {
        let content = std::fs::read_to_string(&path)?;
        let settings: AppSettings = serde_json::from_str(&content)?;
        Ok(settings)
    } else {
        Ok(AppSettings::default())
    }
}

#[cfg(not(target_os = "windows"))]
fn save_impl(settings: &AppSettings) -> Result<()> {
    let path = settings_file_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let payload = serde_json::to_string_pretty(settings)?;
    std::fs::write(path, payload)?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn settings_file_path() -> Result<PathBuf> {
    let base = directories::BaseDirs::new().ok_or(AppSettingsError::MissingSettingsPath)?;
    let mut path = base.home_dir().to_path_buf();
    path.push("Library");
    path.push("Preferences");
    path.push("com.zenithphoto");
    std::fs::create_dir_all(&path)?;
    path.push("settings.json");
    Ok(path)
}

#[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
fn settings_file_path() -> Result<PathBuf> {
    let base = directories::BaseDirs::new().ok_or(AppSettingsError::MissingSettingsPath)?;
    let mut path = base.config_dir().to_path_buf();
    path.push("zenithphoto");
    std::fs::create_dir_all(&path)?;
    path.push("settings.json");
    Ok(path)
}
