use std::path::{Path, PathBuf};

use directories::BaseDirs;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSModalResponseOK, NSOpenPanel};
use objc2_foundation::{NSString, NSURL};

pub fn pick_private_key(current_path: Option<&Path>) -> Result<Option<PathBuf>, String> {
    let main_thread = MainThreadMarker::new()
        .ok_or_else(|| "private-key picker must be opened on the main thread".to_owned())?;
    let base_dirs =
        BaseDirs::new().ok_or_else(|| "failed to resolve the user home directory".to_owned())?;

    let panel = NSOpenPanel::openPanel(main_thread);
    panel.setCanChooseFiles(true);
    panel.setCanChooseDirectories(false);
    panel.setAllowsMultipleSelection(false);
    panel.setShowsHiddenFiles(true);

    let directory = initial_directory(current_path, base_dirs.home_dir());
    let directory = NSString::from_str(directory.to_string_lossy().as_ref());
    let directory_url = NSURL::fileURLWithPath_isDirectory(&directory, true);
    panel.setDirectoryURL(Some(&directory_url));

    if panel.runModal() != NSModalResponseOK {
        return Ok(None);
    }

    let path = panel
        .URL()
        .and_then(|url| url.path())
        .map(|path| PathBuf::from(path.to_string()));

    path.map(Some)
        .ok_or_else(|| "the selected private key has no local file path".to_owned())
}

fn initial_directory(current_path: Option<&Path>, home_dir: &Path) -> PathBuf {
    if let Some(current_path) = current_path {
        let current_path = expand_home_path(current_path, home_dir);

        if current_path.is_dir() {
            return current_path;
        }

        if let Some(parent) = current_path.parent()
            && parent.is_dir()
        {
            return parent.to_path_buf();
        }
    }

    let ssh_directory = home_dir.join(".ssh");
    if ssh_directory.is_dir() {
        ssh_directory
    } else {
        home_dir.to_path_buf()
    }
}

fn expand_home_path(path: &Path, home_dir: &Path) -> PathBuf {
    match path.strip_prefix("~") {
        Ok(relative_path) => home_dir.join(relative_path),
        Err(_) => path.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picker_defaults_to_the_hidden_ssh_directory() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let ssh_directory = directory.path().join(".ssh");
        std::fs::create_dir(&ssh_directory).expect("SSH directory should be created");

        assert_eq!(initial_directory(None, directory.path()), ssh_directory);
    }

    #[test]
    fn picker_uses_the_current_home_relative_key_directory() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let key_directory = directory.path().join("keys");
        std::fs::create_dir(&key_directory).expect("key directory should be created");

        assert_eq!(
            initial_directory(Some(Path::new("~/keys/id_ed25519")), directory.path()),
            key_directory
        );
    }

    #[test]
    fn picker_falls_back_to_home_when_the_ssh_directory_is_missing() {
        let directory = tempfile::tempdir().expect("temporary directory");

        assert_eq!(initial_directory(None, directory.path()), directory.path());
    }
}
