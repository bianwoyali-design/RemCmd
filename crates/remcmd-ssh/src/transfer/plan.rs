use crate::{
    RemoteDirectoryTree,
    remote_path::{
        join_remote_relative, remote_join_path, remote_path_depth, remote_relative_path,
    },
};
use std::path::PathBuf;

pub struct LocalUploadPlan {
    pub directories: Vec<String>,
    pub files: Vec<(PathBuf, String, u64)>,
}

pub fn build_local_upload_plan(
    selected_paths: &[PathBuf],
    remote_directory: &str,
) -> std::io::Result<LocalUploadPlan> {
    let mut directories = Vec::new();
    let mut files = Vec::new();
    let mut pending = Vec::new();

    for path in selected_paths {
        let Some(name) = path.file_name() else {
            continue;
        };
        let remote_path = remote_join_path(remote_directory, name.to_string_lossy().as_ref());
        let metadata = std::fs::symlink_metadata(path)?;
        if metadata.is_dir() {
            directories.push(remote_path.clone());
            pending.push((path.clone(), remote_path));
        } else if metadata.is_file() {
            files.push((path.clone(), remote_path, metadata.len()));
        }
    }

    while let Some((local_directory, remote_directory)) = pending.pop() {
        let mut entries = std::fs::read_dir(&local_directory)?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for entry in entries {
            let local_path = entry.path();
            let metadata = std::fs::symlink_metadata(&local_path)?;
            if metadata.file_type().is_symlink() {
                continue;
            }
            let remote_path = remote_join_path(
                &remote_directory,
                entry.file_name().to_string_lossy().as_ref(),
            );
            if metadata.is_dir() {
                directories.push(remote_path.clone());
                pending.push((local_path, remote_path));
            } else if metadata.is_file() {
                files.push((local_path, remote_path, metadata.len()));
            }
        }
    }

    directories.sort_by(|left, right| {
        remote_path_depth(left)
            .cmp(&remote_path_depth(right))
            .then_with(|| left.cmp(right))
    });
    directories.dedup();
    files.sort_by(|left, right| left.1.cmp(&right.1));
    files.dedup_by(|left, right| left.1 == right.1);
    Ok(LocalUploadPlan { directories, files })
}

pub fn build_remote_download_plan(
    tree: RemoteDirectoryTree,
    destination: PathBuf,
) -> std::io::Result<Vec<(PathBuf, String, Option<u64>)>> {
    std::fs::create_dir_all(&destination)?;
    for directory in tree.directories {
        let relative = remote_relative_path(&tree.root, &directory).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "remote directory escaped its requested root",
            )
        })?;
        std::fs::create_dir_all(join_remote_relative(&destination, relative))?;
    }

    tree.files
        .into_iter()
        .map(|file| {
            let relative = remote_relative_path(&tree.root, &file.path).ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "remote file escaped its requested root",
                )
            })?;
            let local_path = join_remote_relative(&destination, relative);
            if let Some(parent) = local_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            Ok((local_path, file.path, file.size))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RemoteFileEntry, RemoteFileKind};
    #[test]
    fn recursive_upload_plan_preserves_empty_directories_and_files() {
        let temporary = tempfile::tempdir().unwrap();
        let project = temporary.path().join("project");
        std::fs::create_dir_all(project.join("empty")).unwrap();
        std::fs::create_dir_all(project.join("src")).unwrap();
        std::fs::write(project.join("src/main.rs"), "fn main() {}\n").unwrap();

        let plan = build_local_upload_plan(std::slice::from_ref(&project), "/home/test").unwrap();

        assert_eq!(
            plan.directories,
            vec![
                "/home/test/project",
                "/home/test/project/empty",
                "/home/test/project/src",
            ]
        );
        assert_eq!(
            plan.files,
            vec![(
                project.join("src/main.rs"),
                "/home/test/project/src/main.rs".into(),
                13,
            )]
        );
    }

    #[test]
    fn recursive_download_plan_creates_empty_directories_and_file_targets() {
        let temporary = tempfile::tempdir().unwrap();
        let destination = temporary.path().join("project");
        let plan = build_remote_download_plan(
            RemoteDirectoryTree {
                root: "/home/test/project".into(),
                directories: vec![
                    "/home/test/project".into(),
                    "/home/test/project/empty".into(),
                    "/home/test/project/src".into(),
                ],
                files: vec![remote_entry(
                    "/home/test/project/src/main.rs",
                    RemoteFileKind::File,
                )],
            },
            destination.clone(),
        )
        .unwrap();

        assert!(destination.join("empty").is_dir());
        assert!(destination.join("src").is_dir());
        assert_eq!(
            plan,
            vec![(
                destination.join("src/main.rs"),
                "/home/test/project/src/main.rs".into(),
                Some(12),
            )]
        );
    }
    fn remote_entry(path: &str, kind: RemoteFileKind) -> RemoteFileEntry {
        RemoteFileEntry {
            name: crate::remote_path::remote_file_name(path).into(),
            path: path.into(),
            kind,
            size: (kind == RemoteFileKind::File).then_some(12),
            modified: None,
        }
    }
}
