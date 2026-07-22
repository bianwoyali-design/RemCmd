use russh_sftp::{client::SftpSession, protocol::FileType};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

use crate::{ConnectionEvent, SshError, SshErrorKind};

pub const MAX_REMOTE_FILE_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SftpOperation {
    ReadDirectory,
    ReadFile,
    WriteFile,
}

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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteFile {
    pub path: String,
    pub contents: Vec<u8>,
}

enum SftpCommand {
    ReadDirectory {
        request_id: u64,
        path: String,
    },
    ReadFile {
        request_id: u64,
        path: String,
    },
    WriteFile {
        request_id: u64,
        path: String,
        expected_contents: Vec<u8>,
        contents: Vec<u8>,
    },
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
                                operation: SftpOperation::ReadDirectory,
                                error,
                            },
                        };

                        if events.send(event).await.is_err() {
                            break;
                        }
                    }
                    SftpCommand::ReadFile { request_id, path } => {
                        let event = match read_file(&session, path.clone()).await {
                            Ok(file) => ConnectionEvent::FileRead { request_id, file },
                            Err(error) => ConnectionEvent::SftpFailed {
                                request_id,
                                path,
                                operation: SftpOperation::ReadFile,
                                error,
                            },
                        };

                        if events.send(event).await.is_err() {
                            break;
                        }
                    }
                    SftpCommand::WriteFile {
                        request_id,
                        path,
                        expected_contents,
                        contents,
                    } => {
                        let event =
                            match write_file(&session, path.clone(), expected_contents, contents)
                                .await
                            {
                                Ok(file) => ConnectionEvent::FileWritten { request_id, file },
                                Err(error) => ConnectionEvent::SftpFailed {
                                    request_id,
                                    path,
                                    operation: SftpOperation::WriteFile,
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

    pub(crate) fn read_file(&self, request_id: u64, path: String) -> Result<(), SshError> {
        self.command_tx
            .send(SftpCommand::ReadFile { request_id, path })
            .map_err(|_| SshError::new(SshErrorKind::Sftp, "SFTP file worker is not running"))
    }

    pub(crate) fn write_file(
        &self,
        request_id: u64,
        path: String,
        expected_contents: Vec<u8>,
        contents: Vec<u8>,
    ) -> Result<(), SshError> {
        self.command_tx
            .send(SftpCommand::WriteFile {
                request_id,
                path,
                expected_contents,
                contents,
            })
            .map_err(|_| SshError::new(SshErrorKind::Sftp, "SFTP file worker is not running"))
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

async fn read_file(session: &SftpSession, path: String) -> Result<RemoteFile, SshError> {
    let path = session.canonicalize(path).await.map_err(SshError::from)?;
    let metadata = session
        .metadata(path.clone())
        .await
        .map_err(SshError::from)?;
    if metadata
        .size
        .is_some_and(|size| size > MAX_REMOTE_FILE_BYTES as u64)
    {
        return Err(file_too_large_error());
    }

    let file = session.open(path.clone()).await.map_err(SshError::from)?;
    let mut contents = Vec::with_capacity(metadata.size.unwrap_or_default() as usize);
    file.take((MAX_REMOTE_FILE_BYTES + 1) as u64)
        .read_to_end(&mut contents)
        .await
        .map_err(|error| SshError::new(SshErrorKind::Sftp, error.to_string()))?;
    if contents.len() > MAX_REMOTE_FILE_BYTES {
        return Err(file_too_large_error());
    }

    Ok(RemoteFile { path, contents })
}

async fn write_file(
    session: &SftpSession,
    path: String,
    expected_contents: Vec<u8>,
    contents: Vec<u8>,
) -> Result<RemoteFile, SshError> {
    if contents.len() > MAX_REMOTE_FILE_BYTES {
        return Err(file_too_large_error());
    }

    let current = read_file(session, path).await?;
    if current.contents != expected_contents {
        return Err(SshError::new(
            SshErrorKind::Sftp,
            "Remote file changed since it was opened. Reload it before saving.",
        ));
    }

    let mut file = session
        .create(current.path.clone())
        .await
        .map_err(SshError::from)?;
    file.write_all(&contents)
        .await
        .map_err(|error| SshError::new(SshErrorKind::Sftp, error.to_string()))?;
    file.sync_all().await.map_err(SshError::from)?;
    file.shutdown()
        .await
        .map_err(|error| SshError::new(SshErrorKind::Sftp, error.to_string()))?;

    Ok(RemoteFile {
        path: current.path,
        contents,
    })
}

fn file_too_large_error() -> SshError {
    SshError::new(
        SshErrorKind::Sftp,
        format!(
            "Remote file is larger than the {} MB editor limit",
            MAX_REMOTE_FILE_BYTES / 1024 / 1024
        ),
    )
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
    use std::{
        collections::HashMap,
        sync::{Arc, Mutex},
    };

    use russh_sftp::{
        protocol::{
            Attrs, Data, File, FileAttributes, Handle, Name, OpenFlags, Status, StatusCode, Version,
        },
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

    struct TestSftpServer {
        directory_read: bool,
        file_contents: Arc<Mutex<Vec<u8>>>,
    }

    impl Default for TestSftpServer {
        fn default() -> Self {
            Self {
                directory_read: false,
                file_contents: Arc::new(Mutex::new(b"original contents".to_vec())),
            }
        }
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

        async fn realpath(&mut self, id: u32, path: String) -> Result<Name, Self::Error> {
            let path = if path == "." {
                "/home/test".into()
            } else {
                path
            };
            Ok(Name {
                id,
                files: vec![File::dummy(path)],
            })
        }

        async fn stat(&mut self, id: u32, _path: String) -> Result<Attrs, Self::Error> {
            let mut attrs = FileAttributes::default();
            attrs.set_regular(true);
            attrs.size = Some(self.file_contents.lock().unwrap().len() as u64);
            Ok(Attrs { id, attrs })
        }

        async fn open(
            &mut self,
            id: u32,
            filename: String,
            flags: OpenFlags,
            _attrs: FileAttributes,
        ) -> Result<Handle, Self::Error> {
            if flags.contains(OpenFlags::TRUNCATE) {
                self.file_contents.lock().unwrap().clear();
            }
            Ok(Handle {
                id,
                handle: filename,
            })
        }

        async fn read(
            &mut self,
            id: u32,
            _handle: String,
            offset: u64,
            len: u32,
        ) -> Result<Data, Self::Error> {
            let contents = self.file_contents.lock().unwrap();
            let offset = offset as usize;
            if offset >= contents.len() {
                return Err(StatusCode::Eof);
            }
            let end = (offset + len as usize).min(contents.len());
            Ok(Data {
                id,
                data: contents[offset..end].to_vec(),
            })
        }

        async fn write(
            &mut self,
            id: u32,
            _handle: String,
            offset: u64,
            data: Vec<u8>,
        ) -> Result<Status, Self::Error> {
            let mut contents = self.file_contents.lock().unwrap();
            let offset = offset as usize;
            if contents.len() < offset + data.len() {
                contents.resize(offset + data.len(), 0);
            }
            contents[offset..offset + data.len()].copy_from_slice(&data);
            Ok(ok_status(id))
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
            Ok(ok_status(id))
        }
    }

    fn ok_status(id: u32) -> Status {
        Status {
            id,
            status_code: StatusCode::Ok,
            error_message: "Ok".into(),
            language_tag: "en-US".into(),
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

    #[tokio::test]
    async fn reads_a_canonical_remote_file_with_a_size_limit() {
        let server = TestSftpServer::default();
        let expected = server.file_contents.lock().unwrap().clone();
        let (client_stream, server_stream) = tokio::io::duplex(64 * 1024);
        server::run(server_stream, server).await;
        let session = SftpSession::new(client_stream).await.unwrap();

        let file = read_file(&session, "/home/test/notes.txt".into())
            .await
            .unwrap();

        assert_eq!(file.path, "/home/test/notes.txt");
        assert_eq!(file.contents, expected);
    }

    #[tokio::test]
    async fn rejects_a_file_larger_than_the_editor_limit_before_reading_it() {
        let server = TestSftpServer {
            file_contents: Arc::new(Mutex::new(vec![0; MAX_REMOTE_FILE_BYTES + 1])),
            ..TestSftpServer::default()
        };
        let (client_stream, server_stream) = tokio::io::duplex(64 * 1024);
        server::run(server_stream, server).await;
        let session = SftpSession::new(client_stream).await.unwrap();

        let error = read_file(&session, "/home/test/large.txt".into())
            .await
            .expect_err("large file should be rejected");

        assert_eq!(error.kind(), SshErrorKind::Sftp);
        assert!(error.message().contains("editor limit"));
    }

    #[tokio::test]
    async fn refuses_to_overwrite_a_file_changed_after_it_was_read() {
        let server = TestSftpServer::default();
        let shared_contents = server.file_contents.clone();
        let original = shared_contents.lock().unwrap().clone();
        let (client_stream, server_stream) = tokio::io::duplex(64 * 1024);
        server::run(server_stream, server).await;
        let session = SftpSession::new(client_stream).await.unwrap();
        *shared_contents.lock().unwrap() = b"changed elsewhere".to_vec();

        let error = write_file(
            &session,
            "/home/test/notes.txt".into(),
            original,
            b"local edit".to_vec(),
        )
        .await
        .expect_err("conflicting write should be rejected");

        assert!(error.message().contains("changed since it was opened"));
        assert_eq!(&*shared_contents.lock().unwrap(), b"changed elsewhere");
    }

    #[tokio::test]
    async fn saving_a_shorter_file_truncates_the_old_tail() {
        let server = TestSftpServer::default();
        let shared_contents = server.file_contents.clone();
        let original = shared_contents.lock().unwrap().clone();
        let (client_stream, server_stream) = tokio::io::duplex(64 * 1024);
        server::run(server_stream, server).await;
        let session = SftpSession::new(client_stream).await.unwrap();

        let saved = write_file(
            &session,
            "/home/test/notes.txt".into(),
            original,
            b"short".to_vec(),
        )
        .await
        .unwrap();

        assert_eq!(saved.contents, b"short");
        assert_eq!(&*shared_contents.lock().unwrap(), b"short");
    }
}
