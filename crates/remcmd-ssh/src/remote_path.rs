use std::path::{Path, PathBuf};

pub fn remote_parent_path(path: &str) -> Option<String> {
    let path = path.trim_end_matches('/');
    if path.is_empty() || path == "." {
        return None;
    }

    match path.rfind('/') {
        Some(0) => Some("/".into()).filter(|_| path != "/"),
        Some(separator) => Some(path[..separator].into()),
        None => Some(".".into()),
    }
}

pub fn remote_file_name(path: &str) -> &str {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or("download")
}

pub fn remote_join_path(directory: &str, name: &str) -> String {
    if directory == "/" {
        format!("/{name}")
    } else if directory == "." {
        name.to_owned()
    } else {
        format!("{}/{}", directory.trim_end_matches('/'), name)
    }
}

pub fn remote_relative_path<'a>(root: &str, path: &'a str) -> Option<&'a str> {
    if path == root {
        return Some("");
    }
    path.strip_prefix(root.trim_end_matches('/'))?
        .strip_prefix('/')
}

pub fn join_remote_relative(root: &Path, relative: &str) -> PathBuf {
    relative
        .split('/')
        .filter(|component| !component.is_empty() && *component != "." && *component != "..")
        .fold(root.to_path_buf(), |path, component| path.join(component))
}

pub fn remote_path_is_descendant(parent: &str, candidate: &str) -> bool {
    if parent == candidate {
        return false;
    }
    if parent == "/" {
        return candidate.starts_with('/') && candidate.len() > 1;
    }
    candidate
        .strip_prefix(parent.trim_end_matches('/'))
        .is_some_and(|suffix| suffix.starts_with('/'))
}

pub fn remote_path_depth(path: &str) -> usize {
    path.split('/')
        .filter(|component| !component.is_empty())
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn remote_parent_path_handles_root_and_nested_directories() {
        assert_eq!(remote_parent_path("/"), None);
        assert_eq!(remote_parent_path("/home"), Some("/".into()));
        assert_eq!(remote_parent_path("/home/test/"), Some("/home".into()));
        assert_eq!(remote_parent_path("relative"), Some(".".into()));
    }

    #[test]
    fn remote_transfer_paths_join_root_relative_and_nested_directories() {
        assert_eq!(remote_join_path("/", "notes.txt"), "/notes.txt");
        assert_eq!(remote_join_path(".", "notes.txt"), "notes.txt");
        assert_eq!(
            remote_join_path("/home/test/", "notes.txt"),
            "/home/test/notes.txt"
        );
        assert_eq!(remote_file_name("/home/test/notes.txt"), "notes.txt");
    }
}
