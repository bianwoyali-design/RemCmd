use russh_sftp::{client::SftpSession, protocol::FileType};
use tokio::sync::mpsc;

use crate::{ConnectionEvent, SshError, SshErrorKind};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RemoteFileKind {
    Directory,
    File,
    Symlink,
    Other,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteFileEntry {
    pub name: String,
    pub path: String,
    pub kind: RemoteFileKind,
    pub size: Option<u64>,
    pub modified: Option<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteDirectory {
    pub path: String,
    pub entries: Vec<RemoteFileEntry>,
}

enum SftpCommand {
    ReadDirectory { request_id: u64, path: String },
}

pub(crate) struct SftpWorkerHandle {
    command_tx: mpsc::UnboundedSender<SftpCommand>,
}

impl SftpWorkerHandle {
    pub(crate) fn spawn(session: SftpSession, events: mpsc::Sender<ConnectionEvent>) -> Self {
        let (command_tx, mut commands) = mpsc::unbounded_channel();

        tokio::spawn(async move {
            while let Some(command) = commands.recv().await {
                match command {
                    SftpCommand::ReadDirectory { request_id, path } => {
                        let event = match read_directory(&session, path.clone()).await {
                            Ok(directory) => ConnectionEvent::DirectoryRead {
                                request_id,
                                directory,
                            },
                            Err(error) => ConnectionEvent::SftpFailed {
                                request_id,
                                path,
                                error,
                            },
                        };

                        if events.send(event).await.is_err() {
                            break;
                        }
                    }
                }
            }

            let _ = session.close().await;
        });

        Self { command_tx }
    }

    pub(crate) fn read_directory(&self, request_id: u64, path: String) -> Result<(), SshError> {
        self.command_tx
            .send(SftpCommand::ReadDirectory { request_id, path })
            .map_err(|_| SshError::new(SshErrorKind::Sftp, "SFTP directory worker is not running"))
    }
}

async fn read_directory(session: &SftpSession, path: String) -> Result<RemoteDirectory, SshError> {
    let path = session.canonicalize(path).await.map_err(SshError::from)?;
    let entries = session
        .read_dir(path.clone())
        .await
        .map_err(SshError::from)?;
    let mut entries = entries
        .map(|entry| {
            let metadata = entry.metadata();
            RemoteFileEntry {
                name: entry.file_name(),
                path: entry.path(),
                kind: remote_file_kind(entry.file_type()),
                size: metadata.size,
                modified: metadata.mtime,
            }
        })
        .collect::<Vec<_>>();
    sort_entries(&mut entries);

    Ok(RemoteDirectory { path, entries })
}

fn remote_file_kind(kind: FileType) -> RemoteFileKind {
    match kind {
        FileType::Dir => RemoteFileKind::Directory,
        FileType::File => RemoteFileKind::File,
        FileType::Symlink => RemoteFileKind::Symlink,
        FileType::Other => RemoteFileKind::Other,
    }
}

fn sort_entries(entries: &mut [RemoteFileEntry]) {
    entries.sort_by(|left, right| {
        file_kind_rank(left.kind)
            .cmp(&file_kind_rank(right.kind))
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
            .then_with(|| left.name.cmp(&right.name))
    });
}

const fn file_kind_rank(kind: RemoteFileKind) -> u8 {
    match kind {
        RemoteFileKind::Directory => 0,
        RemoteFileKind::Symlink => 1,
        RemoteFileKind::File => 2,
        RemoteFileKind::Other => 3,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use russh_sftp::{
        protocol::{File, FileAttributes, Handle, Name, Status, StatusCode, Version},
        server,
    };

    use super::*;

    fn entry(name: &str, kind: RemoteFileKind) -> RemoteFileEntry {
        RemoteFileEntry {
            name: name.into(),
            path: format!("/home/test/{name}"),
            kind,
            size: None,
            modified: None,
        }
    }

    #[test]
    fn directory_entries_sort_by_kind_then_name() {
        let mut entries = vec![
            entry("z.txt", RemoteFileKind::File),
            entry("beta", RemoteFileKind::Directory),
            entry("Alpha", RemoteFileKind::Directory),
            entry("link", RemoteFileKind::Symlink),
        ];

        sort_entries(&mut entries);

        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Alpha", "beta", "link", "z.txt"]
        );
    }

    #[derive(Default)]
    struct TestSftpServer {
        directory_read: bool,
    }

    impl server::Handler for TestSftpServer {
        type Error = StatusCode;

        fn unimplemented(&self) -> Self::Error {
            StatusCode::OpUnsupported
        }

        async fn init(
            &mut self,
            _version: u32,
            _extensions: HashMap<String, String>,
        ) -> Result<Version, Self::Error> {
            Ok(Version::new())
        }

        async fn realpath(&mut self, id: u32, _path: String) -> Result<Name, Self::Error> {
            Ok(Name {
                id,
                files: vec![File::dummy("/home/test")],
            })
        }

        async fn opendir(&mut self, id: u32, path: String) -> Result<Handle, Self::Error> {
            self.directory_read = false;
            Ok(Handle { id, handle: path })
        }

        async fn readdir(&mut self, id: u32, _handle: String) -> Result<Name, Self::Error> {
            if self.directory_read {
                return Err(StatusCode::Eof);
            }
            self.directory_read = true;

            let mut directory = FileAttributes::default();
            directory.set_dir(true);
            let mut file = FileAttributes::default();
            file.set_regular(true);
            file.size = Some(1536);
            file.mtime = Some(1_700_000_000);

            Ok(Name {
                id,
                files: vec![
                    File::new("notes.txt", file),
                    File::new("projects", directory),
                ],
            })
        }

        async fn close(&mut self, id: u32, _handle: String) -> Result<Status, Self::Error> {
            Ok(Status {
                id,
                status_code: StatusCode::Ok,
                error_message: "Ok".into(),
                language_tag: "en-US".into(),
            })
        }
    }

    #[tokio::test]
    async fn reads_and_maps_a_remote_directory_over_sftp() {
        let (client_stream, server_stream) = tokio::io::duplex(64 * 1024);
        server::run(server_stream, TestSftpServer::default()).await;
        let session = SftpSession::new(client_stream)
            .await
            .expect("SFTP client should initialize");

        let directory = read_directory(&session, ".".into())
            .await
            .expect("directory should be read");

        assert_eq!(directory.path, "/home/test");
        assert_eq!(directory.entries.len(), 2);
        assert_eq!(directory.entries[0].name, "projects");
        assert_eq!(directory.entries[0].kind, RemoteFileKind::Directory);
        assert_eq!(directory.entries[1].path, "/home/test/notes.txt");
        assert_eq!(directory.entries[1].size, Some(1536));
        assert_eq!(directory.entries[1].modified, Some(1_700_000_000));
    }
}
