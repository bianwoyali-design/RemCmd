use std::{
    fs, io,
    path::{Path, PathBuf},
};

use directories::ProjectDirs;
use remcmd_core::ConnectionProfile;

pub fn default_profiles_path() -> io::Result<PathBuf> {
    let project_dirs = ProjectDirs::from("", "", "RemCmd")
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "app data directory not found"))?;

    Ok(project_dirs.data_dir().join("profiles.json"))
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
