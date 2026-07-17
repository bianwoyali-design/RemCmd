use std::{
    fs, io,
    path::{Path, PathBuf},
};

use directories::ProjectDirs;
use remcmd_core::{ConnectionProfile, ThemeMode};

pub fn default_profiles_path() -> io::Result<PathBuf> {
    let project_dirs = ProjectDirs::from("", "", "RemCmd")
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "app data directory not found"))?;

    Ok(project_dirs.data_dir().join("profiles.json"))
}

pub fn default_settings_path() -> io::Result<PathBuf> {
    let project_dirs = ProjectDirs::from("", "", "RemCmd")
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "app data directory not found"))?;

    Ok(project_dirs.data_dir().join("settings.json"))
}

#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Deserialize, serde::Serialize)]
pub struct AppSettings {
    #[serde(default)]
    pub theme_mode: ThemeMode,
}

pub fn ensure_profiles_file(path: &Path) -> io::Result<()> {
    if path.exists() {
        if path.is_file() {
            return Ok(());
        }

        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "profiles path exists but is not a file",
        ));
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    fs::write(path, "[]")?;

    Ok(())
}

pub fn load_profiles(path: &Path) -> io::Result<Vec<ConnectionProfile>> {
    let content = fs::read_to_string(path)?;

    serde_json::from_str(&content)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

pub fn save_profiles(path: &Path, profiles: &[ConnectionProfile]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let content = serde_json::to_string_pretty(profiles).map_err(io::Error::other)?;
    fs::write(path, content)
}

pub fn load_settings(path: &Path) -> io::Result<AppSettings> {
    if !path.exists() {
        return Ok(AppSettings::default());
    }

    let content = fs::read_to_string(path)?;

    serde_json::from_str(&content)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

pub fn save_settings(path: &Path, settings: &AppSettings) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let content = serde_json::to_string_pretty(settings).map_err(io::Error::other)?;
    fs::write(path, content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_settings_file_uses_system_theme() {
        let directory = tempfile::tempdir().unwrap();
        let settings = load_settings(&directory.path().join("settings.json")).unwrap();

        assert_eq!(settings.theme_mode, ThemeMode::System);
    }

    #[test]
    fn settings_round_trip_creates_parent_directory() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("nested/settings.json");
        let settings = AppSettings {
            theme_mode: ThemeMode::Dark,
        };

        save_settings(&path, &settings).unwrap();

        assert_eq!(load_settings(&path).unwrap(), settings);
    }

    #[test]
    fn invalid_settings_are_reported_as_invalid_data() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        fs::write(&path, "not json").unwrap();

        assert_eq!(
            load_settings(&path).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }
}
