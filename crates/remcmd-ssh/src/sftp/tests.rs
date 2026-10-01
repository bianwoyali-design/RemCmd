use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex, atomic::AtomicBool},
};

use russh_sftp::{
    protocol::{
        Attrs, Data, File, FileAttributes, Handle, Name, OpenFlags, Status, StatusCode, Version,
    },
    server,
};

use super::*;
use crate::remote_files::{FileBackend, FileCommand, FileWorkerHandle};
use crate::{ConnectionEvent, TransferRateLimiter};
use tokio::{
    sync::mpsc,
    time::{Duration, timeout},
};

fn entry(name: &str, kind: RemoteFileKind) -> RemoteFileEntry {
    RemoteFileEntry {
        name: name.into(),
        path: format!("/home/test/{name}"),
        kind,
        size: None,
        modified: None,
    }
}

fn transfer_context(
    transfer_id: u64,
    cancellation: Arc<AtomicBool>,
    events: &mpsc::Sender<ConnectionEvent>,
) -> TransferContext {
    TransferContext::new(
        transfer_id,
        cancellation,
        events.clone(),
        Arc::new(TransferRateLimiter::default()),
    )
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

#[test]
fn large_downloads_are_split_into_up_to_four_contiguous_ranges() {
    assert_eq!(download_segment_ranges(0), Vec::<Range<u64>>::new());
    assert_eq!(
        download_segment_ranges(MIN_DOWNLOAD_SEGMENT_BYTES),
        vec![0..MIN_DOWNLOAD_SEGMENT_BYTES]
    );

    let total = MIN_DOWNLOAD_SEGMENT_BYTES * 4 + 17;
    let ranges = download_segment_ranges(total);

    assert_eq!(ranges.len(), DOWNLOAD_PIPELINE_STREAMS);
    assert_eq!(ranges.first().unwrap().start, 0);
    assert_eq!(ranges.last().unwrap().end, total);
    assert!(ranges.windows(2).all(|pair| pair[0].end == pair[1].start));
}

struct TestSftpServer {
    directory_reads: HashSet<String>,
    directories: Arc<Mutex<HashSet<String>>>,
    files: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    read_offsets: Arc<Mutex<Vec<u64>>>,
}

impl Default for TestSftpServer {
    fn default() -> Self {
        let files = HashMap::from([("/home/test/notes.txt".into(), b"original contents".to_vec())]);
        Self {
            directory_reads: HashSet::new(),
            directories: Arc::new(Mutex::new(HashSet::from([
                "/home/test".into(),
                "/home/test/projects".into(),
            ]))),
            files: Arc::new(Mutex::new(files)),
            read_offsets: Arc::new(Mutex::new(Vec::new())),
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

    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        if self.directories.lock().unwrap().contains(&path) {
            let mut attrs = FileAttributes::default();
            attrs.set_dir(true);
            return Ok(Attrs { id, attrs });
        }
        let files = self.files.lock().unwrap();
        let Some(contents) = files.get(&path) else {
            return Err(StatusCode::NoSuchFile);
        };
        let mut attrs = FileAttributes::default();
        attrs.set_regular(true);
        attrs.size = Some(contents.len() as u64);
        Ok(Attrs { id, attrs })
    }

    async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        if self.directories.lock().unwrap().contains(&path) {
            let mut attrs = FileAttributes::default();
            attrs.set_dir(true);
            return Ok(Attrs { id, attrs });
        }
        let files = self.files.lock().unwrap();
        let Some(contents) = files.get(&path) else {
            return Err(StatusCode::NoSuchFile);
        };
        let mut attrs = FileAttributes::default();
        attrs.set_regular(true);
        attrs.size = Some(contents.len() as u64);
        Ok(Attrs { id, attrs })
    }

    async fn open(
        &mut self,
        id: u32,
        filename: String,
        flags: OpenFlags,
        _attrs: FileAttributes,
    ) -> Result<Handle, Self::Error> {
        let mut files = self.files.lock().unwrap();
        if flags.contains(OpenFlags::EXCLUDE) && files.contains_key(&filename) {
            return Err(StatusCode::Failure);
        }
        if flags.contains(OpenFlags::TRUNCATE) || flags.contains(OpenFlags::CREATE) {
            files.insert(filename.clone(), Vec::new());
        } else if !files.contains_key(&filename) {
            return Err(StatusCode::NoSuchFile);
        }
        Ok(Handle {
            id,
            handle: filename,
        })
    }

    async fn read(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        len: u32,
    ) -> Result<Data, Self::Error> {
        self.read_offsets.lock().unwrap().push(offset);
        let files = self.files.lock().unwrap();
        let Some(contents) = files.get(&handle) else {
            return Err(StatusCode::NoSuchFile);
        };
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
        handle: String,
        offset: u64,
        data: Vec<u8>,
    ) -> Result<Status, Self::Error> {
        let mut files = self.files.lock().unwrap();
        let contents = files.entry(handle).or_default();
        let offset = offset as usize;
        if contents.len() < offset + data.len() {
            contents.resize(offset + data.len(), 0);
        }
        contents[offset..offset + data.len()].copy_from_slice(&data);
        Ok(ok_status(id))
    }

    async fn remove(&mut self, id: u32, filename: String) -> Result<Status, Self::Error> {
        if self.files.lock().unwrap().remove(&filename).is_none() {
            return Err(StatusCode::NoSuchFile);
        }
        Ok(ok_status(id))
    }

    async fn mkdir(
        &mut self,
        id: u32,
        path: String,
        _attrs: FileAttributes,
    ) -> Result<Status, Self::Error> {
        let mut directories = self.directories.lock().unwrap();
        if !directories.insert(path) {
            return Err(StatusCode::Failure);
        }
        Ok(ok_status(id))
    }

    async fn rmdir(&mut self, id: u32, path: String) -> Result<Status, Self::Error> {
        let prefix = format!("{}/", path.trim_end_matches('/'));
        if self
            .files
            .lock()
            .unwrap()
            .keys()
            .any(|file| file.starts_with(&prefix))
            || self
                .directories
                .lock()
                .unwrap()
                .iter()
                .any(|directory| directory != &path && directory.starts_with(&prefix))
        {
            return Err(StatusCode::Failure);
        }
        if !self.directories.lock().unwrap().remove(&path) {
            return Err(StatusCode::NoSuchFile);
        }
        Ok(ok_status(id))
    }

    async fn rename(
        &mut self,
        id: u32,
        oldpath: String,
        newpath: String,
    ) -> Result<Status, Self::Error> {
        let mut files = self.files.lock().unwrap();
        let Some(contents) = files.remove(&oldpath) else {
            return Err(StatusCode::NoSuchFile);
        };
        files.insert(newpath, contents);
        Ok(ok_status(id))
    }

    async fn opendir(&mut self, id: u32, path: String) -> Result<Handle, Self::Error> {
        if !self.directories.lock().unwrap().contains(&path) {
            return Err(StatusCode::NoSuchFile);
        }
        self.directory_reads.remove(&path);
        Ok(Handle { id, handle: path })
    }

    async fn readdir(&mut self, id: u32, handle: String) -> Result<Name, Self::Error> {
        if !self.directory_reads.insert(handle.clone()) {
            return Err(StatusCode::Eof);
        }

        let prefix = format!("{}/", handle.trim_end_matches('/'));
        let mut files = self
            .directories
            .lock()
            .unwrap()
            .iter()
            .filter_map(|path| {
                let name = path.strip_prefix(&prefix)?;
                (!name.is_empty() && !name.contains('/')).then(|| {
                    let mut attrs = FileAttributes::default();
                    attrs.set_dir(true);
                    File::new(name, attrs)
                })
            })
            .collect::<Vec<_>>();
        files.extend(
            self.files
                .lock()
                .unwrap()
                .iter()
                .filter_map(|(path, contents)| {
                    let name = path.strip_prefix(&prefix)?;
                    (!name.is_empty() && !name.contains('/')).then(|| {
                        let mut attrs = FileAttributes::default();
                        attrs.set_regular(true);
                        attrs.size = Some(contents.len() as u64);
                        attrs.mtime = Some(1_700_000_000);
                        File::new(name, attrs)
                    })
                }),
        );

        Ok(Name { id, files })
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
    assert_eq!(
        directory.entries[1].size,
        Some(b"original contents".len() as u64)
    );
    assert_eq!(directory.entries[1].modified, Some(1_700_000_000));
}

#[tokio::test]
async fn recursively_reads_regular_files_and_preserves_empty_directories() {
    let server = TestSftpServer::default();
    server
        .directories
        .lock()
        .unwrap()
        .insert("/home/test/projects/src".into());
    server.files.lock().unwrap().extend([
        ("/home/test/projects/todo.txt".into(), b"todo".to_vec()),
        (
            "/home/test/projects/src/main.rs".into(),
            b"fn main() {}".to_vec(),
        ),
    ]);
    let (client_stream, server_stream) = tokio::io::duplex(64 * 1024);
    server::run(server_stream, server).await;
    let session = SftpSession::new(client_stream).await.unwrap();

    let tree = read_directory_tree(&session, "/home/test".into())
        .await
        .unwrap();

    assert_eq!(tree.root, "/home/test");
    assert_eq!(
        tree.directories,
        vec![
            "/home/test/projects".to_owned(),
            "/home/test/projects/src".to_owned()
        ]
    );
    assert_eq!(
        tree.files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>(),
        vec![
            "/home/test/notes.txt",
            "/home/test/projects/src/main.rs",
            "/home/test/projects/todo.txt"
        ]
    );
}

#[tokio::test]
async fn creates_and_recursively_deletes_remote_items() {
    let server = TestSftpServer::default();
    let directories = server.directories.clone();
    let files = server.files.clone();
    let (client_stream, server_stream) = tokio::io::duplex(64 * 1024);
    server::run(server_stream, server).await;
    let session = SftpSession::new(client_stream).await.unwrap();

    create_directories(
        &session,
        vec!["/home/test/new/nested".into(), "/home/test/new".into()],
    )
    .await
    .unwrap();
    let path = create_file(&session, "/home/test/new/nested/empty.txt".into())
        .await
        .unwrap();

    assert_eq!(path, "/home/test/new/nested/empty.txt");
    assert!(files.lock().unwrap().contains_key(&path));
    assert!(
        create_file(&session, "/home/test/new/nested/empty.txt".into())
            .await
            .is_err()
    );
    assert!(
        directories
            .lock()
            .unwrap()
            .contains("/home/test/new/nested")
    );

    delete_paths(&session, &["/home/test/new".into()])
        .await
        .unwrap();

    assert!(!files.lock().unwrap().contains_key(&path));
    assert!(
        !directories
            .lock()
            .unwrap()
            .iter()
            .any(|path| path.starts_with("/home/test/new"))
    );
}

#[tokio::test]
async fn reads_a_canonical_remote_file_with_a_size_limit() {
    let server = TestSftpServer::default();
    let expected = server.files.lock().unwrap()["/home/test/notes.txt"].clone();
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
        files: Arc::new(Mutex::new(HashMap::from([(
            "/home/test/large.txt".into(),
            vec![0; MAX_REMOTE_FILE_BYTES + 1],
        )]))),
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
    let shared_files = server.files.clone();
    let original = shared_files.lock().unwrap()["/home/test/notes.txt"].clone();
    let (client_stream, server_stream) = tokio::io::duplex(64 * 1024);
    server::run(server_stream, server).await;
    let session = SftpSession::new(client_stream).await.unwrap();
    shared_files
        .lock()
        .unwrap()
        .insert("/home/test/notes.txt".into(), b"changed elsewhere".to_vec());

    let error = write_file(
        &session,
        "/home/test/notes.txt".into(),
        original,
        b"local edit".to_vec(),
    )
    .await
    .expect_err("conflicting write should be rejected");

    assert!(error.message().contains("changed since it was opened"));
    assert_eq!(
        &shared_files.lock().unwrap()["/home/test/notes.txt"],
        b"changed elsewhere"
    );
}

#[tokio::test]
async fn saving_a_shorter_file_truncates_the_old_tail() {
    let server = TestSftpServer::default();
    let shared_files = server.files.clone();
    let original = shared_files.lock().unwrap()["/home/test/notes.txt"].clone();
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
    assert_eq!(
        &shared_files.lock().unwrap()["/home/test/notes.txt"],
        b"short"
    );
}

#[tokio::test]
async fn uploads_through_a_temporary_remote_file_and_reports_progress() {
    let server = TestSftpServer::default();
    let shared_files = server.files.clone();
    let (client_stream, server_stream) = tokio::io::duplex(512 * 1024);
    server::run(server_stream, server).await;
    let session = SftpSession::new(client_stream).await.unwrap();
    let directory = tempfile::tempdir().unwrap();
    let local_path = directory.path().join("upload.bin");
    let contents = vec![0x5a; TRANSFER_CHUNK_BYTES + 17];
    fs::write(&local_path, &contents).await.unwrap();
    let cancellation = Arc::new(AtomicBool::new(false));
    let (event_tx, mut event_rx) = mpsc::channel(8);
    let context = transfer_context(41, cancellation, &event_tx);

    let result = upload_file(
        &session,
        &local_path,
        "/home/test/upload.bin".into(),
        false,
        &context,
    )
    .await
    .unwrap();

    assert!(matches!(
        result,
        TransferResult::Completed(bytes) if bytes == contents.len() as u64
    ));
    let files = shared_files.lock().unwrap();
    assert_eq!(files["/home/test/upload.bin"], contents);
    assert!(!files.keys().any(|path| path.ends_with(".part")));
    drop(files);
    let mut progress = Vec::new();
    while let Ok(event) = event_rx.try_recv() {
        if let ConnectionEvent::TransferProgress { transferred, .. } = event {
            progress.push(transferred);
        }
    }
    assert_eq!(progress.last().copied(), Some(contents.len() as u64));
}

#[tokio::test]
async fn downloads_through_a_temporary_local_file() {
    let server = TestSftpServer::default();
    let expected = server.files.lock().unwrap()["/home/test/notes.txt"].clone();
    let (client_stream, server_stream) = tokio::io::duplex(64 * 1024);
    server::run(server_stream, server).await;
    let session = Arc::new(SftpSession::new(client_stream).await.unwrap());
    let directory = tempfile::tempdir().unwrap();
    let local_path = directory.path().join("notes.txt");
    let cancellation = Arc::new(AtomicBool::new(false));
    let (event_tx, _event_rx) = mpsc::channel(8);
    let context = transfer_context(42, cancellation, &event_tx);

    let result = download_file(
        session,
        "/home/test/notes.txt".into(),
        &local_path,
        false,
        &context,
    )
    .await
    .unwrap();

    assert!(matches!(
        result,
        TransferResult::Completed(bytes) if bytes == expected.len() as u64
    ));
    assert_eq!(fs::read(&local_path).await.unwrap(), expected);
    assert!(std::fs::read_dir(directory.path()).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".remcmd-")
    }));
}

#[tokio::test]
async fn large_downloads_use_multiple_ranged_sftp_streams() {
    let contents = (0..(MIN_DOWNLOAD_SEGMENT_BYTES * 4 + 17))
        .map(|index| (index % 251) as u8)
        .collect::<Vec<_>>();
    let server = TestSftpServer {
        files: Arc::new(Mutex::new(HashMap::from([(
            "/home/test/large.bin".into(),
            contents.clone(),
        )]))),
        ..TestSftpServer::default()
    };
    let read_offsets = server.read_offsets.clone();
    let (client_stream, server_stream) = tokio::io::duplex(512 * 1024);
    server::run(server_stream, server).await;
    let session = Arc::new(SftpSession::new(client_stream).await.unwrap());
    let directory = tempfile::tempdir().unwrap();
    let local_path = directory.path().join("large.bin");
    let (event_tx, _event_rx) = mpsc::channel(64);
    let total = contents.len() as u64;
    let expected_ranges = download_segment_ranges(total);
    let context = transfer_context(49, Arc::new(AtomicBool::new(false)), &event_tx);

    let result = download_file(
        session,
        "/home/test/large.bin".into(),
        &local_path,
        false,
        &context,
    )
    .await
    .unwrap();

    assert!(matches!(result, TransferResult::Completed(bytes) if bytes == total));
    assert_eq!(fs::read(local_path).await.unwrap(), contents);
    let read_offsets = read_offsets.lock().unwrap();
    assert!(
        expected_ranges
            .iter()
            .all(|range| read_offsets.contains(&range.start)),
        "each ranged stream should read from its own starting offset"
    );
}

#[tokio::test]
async fn interrupted_parallel_downloads_keep_the_destination_and_remove_partial_files() {
    for fail_read in [false, true] {
        let server = TestSftpServer::default();
        let files = server.files.clone();
        files.lock().unwrap().insert(
            "/home/test/large.bin".into(),
            vec![0x5a; (MIN_DOWNLOAD_SEGMENT_BYTES * 4 + 17) as usize],
        );
        let (client_stream, server_stream) = tokio::io::duplex(512 * 1024);
        server::run(server_stream, server).await;
        let session = Arc::new(SftpSession::new(client_stream).await.unwrap());
        let directory = tempfile::tempdir().unwrap();
        let local_path = directory.path().join("large.bin");
        fs::write(&local_path, b"keep destination").await.unwrap();
        let cancellation = Arc::new(AtomicBool::new(false));
        let (event_tx, mut event_rx) = mpsc::channel(64);
        let context = transfer_context(50, cancellation.clone(), &event_tx);

        let transfer = download_file(
            session,
            "/home/test/large.bin".into(),
            &local_path,
            true,
            &context,
        );
        let interrupt_after_progress = async {
            assert!(matches!(
                event_rx.recv().await,
                Some(ConnectionEvent::TransferProgress { .. })
            ));
            if fail_read {
                files.lock().unwrap().clear();
            } else {
                cancellation.store(true, Ordering::Release);
            }
        };
        let (result, ()) = timeout(Duration::from_secs(2), async {
            tokio::join!(transfer, interrupt_after_progress)
        })
        .await
        .expect("interrupted segments should stop and release their local file handles");
        if fail_read {
            assert_eq!(result.err().unwrap().kind(), SshErrorKind::Sftp);
        } else {
            assert!(matches!(result.unwrap(), TransferResult::Cancelled));
        }
        assert_eq!(fs::read(&local_path).await.unwrap(), b"keep destination");
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }
}

#[tokio::test]
async fn transfer_conflicts_do_not_replace_existing_files() {
    let server = TestSftpServer::default();
    let shared_files = server.files.clone();
    let original_remote = shared_files.lock().unwrap()["/home/test/notes.txt"].clone();
    let (client_stream, server_stream) = tokio::io::duplex(64 * 1024);
    server::run(server_stream, server).await;
    let session = Arc::new(SftpSession::new(client_stream).await.unwrap());
    let directory = tempfile::tempdir().unwrap();
    let upload_path = directory.path().join("upload.txt");
    fs::write(&upload_path, b"replacement").await.unwrap();
    let download_path = directory.path().join("download.txt");
    fs::write(&download_path, b"keep local").await.unwrap();
    let cancellation = Arc::new(AtomicBool::new(false));
    let (event_tx, _event_rx) = mpsc::channel(8);
    let upload_context = transfer_context(43, cancellation.clone(), &event_tx);
    let download_context = transfer_context(44, cancellation, &event_tx);

    let upload = upload_file(
        &session,
        &upload_path,
        "/home/test/notes.txt".into(),
        false,
        &upload_context,
    )
    .await
    .unwrap();
    let download = download_file(
        session,
        "/home/test/notes.txt".into(),
        &download_path,
        false,
        &download_context,
    )
    .await
    .unwrap();

    assert!(matches!(upload, TransferResult::Conflict));
    assert!(matches!(download, TransferResult::Conflict));
    assert_eq!(
        shared_files.lock().unwrap()["/home/test/notes.txt"],
        original_remote
    );
    assert_eq!(fs::read(download_path).await.unwrap(), b"keep local");
}

#[tokio::test]
async fn confirmed_transfers_replace_existing_destinations() {
    let server = TestSftpServer::default();
    let shared_files = server.files.clone();
    let (client_stream, server_stream) = tokio::io::duplex(64 * 1024);
    server::run(server_stream, server).await;
    let session = Arc::new(SftpSession::new(client_stream).await.unwrap());
    let directory = tempfile::tempdir().unwrap();
    let upload_path = directory.path().join("upload.txt");
    fs::write(&upload_path, b"remote replacement")
        .await
        .unwrap();
    let download_path = directory.path().join("download.txt");
    fs::write(&download_path, b"old local").await.unwrap();
    let cancellation = Arc::new(AtomicBool::new(false));
    let (event_tx, _event_rx) = mpsc::channel(8);
    let upload_context = transfer_context(45, cancellation.clone(), &event_tx);
    let download_context = transfer_context(46, cancellation, &event_tx);

    let upload = upload_file(
        &session,
        &upload_path,
        "/home/test/notes.txt".into(),
        true,
        &upload_context,
    )
    .await
    .unwrap();
    let download = download_file(
        session,
        "/home/test/notes.txt".into(),
        &download_path,
        true,
        &download_context,
    )
    .await
    .unwrap();

    assert!(matches!(upload, TransferResult::Completed(18)));
    assert!(matches!(download, TransferResult::Completed(18)));
    assert_eq!(
        shared_files.lock().unwrap()["/home/test/notes.txt"],
        b"remote replacement"
    );
    assert_eq!(
        fs::read(download_path).await.unwrap(),
        b"remote replacement"
    );
}

#[tokio::test]
async fn cancelled_transfer_does_not_create_a_partial_destination() {
    let server = TestSftpServer::default();
    let shared_files = server.files.clone();
    let (client_stream, server_stream) = tokio::io::duplex(64 * 1024);
    server::run(server_stream, server).await;
    let session = SftpSession::new(client_stream).await.unwrap();
    let directory = tempfile::tempdir().unwrap();
    let local_path = directory.path().join("cancelled.txt");
    fs::write(&local_path, b"cancel me").await.unwrap();
    let cancellation = Arc::new(AtomicBool::new(true));
    let (event_tx, _event_rx) = mpsc::channel(8);
    let context = transfer_context(45, cancellation, &event_tx);

    let result = upload_file(
        &session,
        &local_path,
        "/home/test/cancelled.txt".into(),
        false,
        &context,
    )
    .await
    .unwrap();

    assert!(matches!(result, TransferResult::Cancelled));
    assert!(
        !shared_files
            .lock()
            .unwrap()
            .contains_key("/home/test/cancelled.txt")
    );
}

#[tokio::test]
async fn active_transfer_cancellation_removes_the_temporary_file() {
    let server = TestSftpServer::default();
    let shared_files = server.files.clone();
    let (client_stream, server_stream) = tokio::io::duplex(512 * 1024);
    server::run(server_stream, server).await;
    let session = SftpSession::new(client_stream).await.unwrap();
    let directory = tempfile::tempdir().unwrap();
    let local_path = directory.path().join("cancelled-active.bin");
    fs::write(&local_path, vec![0x5a; TRANSFER_CHUNK_BYTES * 3])
        .await
        .unwrap();
    let cancellation = Arc::new(AtomicBool::new(false));
    let (event_tx, mut event_rx) = mpsc::channel(1);
    let context = transfer_context(47, cancellation.clone(), &event_tx);

    let transfer = upload_file(
        &session,
        &local_path,
        "/home/test/cancelled-active.bin".into(),
        false,
        &context,
    );
    let cancel_after_progress = async {
        assert!(matches!(
            event_rx.recv().await,
            Some(ConnectionEvent::TransferProgress { .. })
        ));
        cancellation.store(true, Ordering::Release);
    };
    let (result, ()) = tokio::join!(transfer, cancel_after_progress);

    assert!(matches!(result.unwrap(), TransferResult::Cancelled));
    assert!(
        !shared_files
            .lock()
            .unwrap()
            .keys()
            .any(|path| path.contains("cancelled-active"))
    );
}

#[tokio::test]
async fn dropping_the_worker_cancels_and_cleans_active_transfers() {
    let server = TestSftpServer::default();
    let shared_files = server.files.clone();
    let (client_stream, server_stream) = tokio::io::duplex(512 * 1024);
    server::run(server_stream, server).await;
    let session = SftpSession::new(client_stream).await.unwrap();
    let directory = tempfile::tempdir().unwrap();
    let local_path = directory.path().join("worker-drop.bin");
    fs::write(&local_path, vec![0x5a; TRANSFER_CHUNK_BYTES * 3])
        .await
        .unwrap();
    let (event_tx, mut event_rx) = mpsc::channel(8);
    let worker = FileWorkerHandle::spawn(
        FileBackend::Sftp(Arc::new(session)),
        event_tx,
        Arc::new(TransferRateLimiter::default()),
    );

    worker
        .send(FileCommand::Transfer {
            transfer_id: 48,
            direction: crate::SftpTransferDirection::Upload,
            local_path,
            remote_path: "/home/test/worker-drop.bin".into(),
            overwrite: false,
        })
        .unwrap();
    drop(worker);

    timeout(Duration::from_secs(1), async {
        loop {
            match event_rx.recv().await {
                Some(ConnectionEvent::TransferCancelled { transfer_id: 48 }) => break,
                Some(_) => {}
                None => panic!("worker should report transfer cancellation"),
            }
        }
    })
    .await
    .expect("worker should finish the cancelled transfer");
    assert!(
        !shared_files
            .lock()
            .unwrap()
            .keys()
            .any(|path| path.contains("worker-drop"))
    );
}

#[tokio::test]
async fn file_worker_preserves_request_order_and_continues_after_a_write_conflict() {
    let (client_stream, server_stream) = tokio::io::duplex(64 * 1024);
    server::run(server_stream, TestSftpServer::default()).await;
    let session = SftpSession::new(client_stream).await.unwrap();
    let (event_tx, mut event_rx) = mpsc::channel(8);
    let worker = FileWorkerHandle::spawn(
        FileBackend::Sftp(Arc::new(session)),
        event_tx,
        Arc::new(TransferRateLimiter::default()),
    );
    let path = "/home/test/notes.txt".to_owned();
    let contents = b"edited contents".to_vec();
    let contents_pointer = contents.as_ptr() as usize;
    for command in [
        FileCommand::WriteFile {
            request_id: 1,
            path: path.clone(),
            expected_contents: b"stale contents".to_vec(),
            contents: b"rejected".to_vec(),
        },
        FileCommand::WriteFile {
            request_id: 2,
            path: path.clone(),
            expected_contents: b"original contents".to_vec(),
            contents,
        },
        FileCommand::ReadFile {
            request_id: 3,
            path: path.clone(),
        },
    ] {
        worker.send(command).unwrap();
    }
    timeout(Duration::from_secs(2), async {
        assert!(matches!(
            event_rx.recv().await,
            Some(ConnectionEvent::SftpFailed {
                request_id: 1,
                operation: crate::SftpOperation::WriteFile,
                ..
            })
        ));
        let Some(ConnectionEvent::FileWritten {
            request_id: 2,
            file,
        }) = event_rx.recv().await
        else {
            panic!("worker should continue after a conflict");
        };
        assert_eq!(file.contents, b"edited contents");
        assert_eq!(
            file.contents.as_ptr() as usize,
            contents_pointer,
            "file contents should move through the worker without a second buffer"
        );
        assert_eq!(
            event_rx.recv().await,
            Some(ConnectionEvent::FileRead {
                request_id: 3,
                file
            })
        );
    })
    .await
    .expect("queued requests should finish");
}
