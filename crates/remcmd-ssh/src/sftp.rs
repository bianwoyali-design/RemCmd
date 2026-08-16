use std::{
    collections::HashMap,
    ops::Range,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use russh_sftp::{
    client::SftpSession,
    protocol::{FileType, OpenFlags},
};
use tokio::sync::{Mutex as AsyncMutex, mpsc};
use tokio::{
    fs,
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
    task::JoinSet,
    time::{Duration, sleep, timeout},
};

use crate::{ConnectionEvent, SshError, SshErrorKind};

pub const MAX_REMOTE_FILE_BYTES: usize = 2 * 1024 * 1024;

pub struct TransferRateLimiter {
    bytes_per_second: AtomicU64,
    pacing: AsyncMutex<()>,
}

impl TransferRateLimiter {
    pub fn new(bytes_per_second: Option<u64>) -> Self {
        Self {
            bytes_per_second: AtomicU64::new(bytes_per_second.unwrap_or(0)),
            pacing: AsyncMutex::new(()),
        }
    }

    pub fn set_bytes_per_second(&self, bytes_per_second: Option<u64>) {
        self.bytes_per_second
            .store(bytes_per_second.unwrap_or(0), Ordering::Release);
    }

    async fn acquire(&self, bytes: usize) {
        if bytes == 0 || self.bytes_per_second.load(Ordering::Acquire) == 0 {
            return;
        }

        let _pacing = self.pacing.lock().await;
        let bytes_per_second = self.bytes_per_second.load(Ordering::Acquire);
        if bytes_per_second == 0 {
            return;
        }
        sleep(Duration::from_secs_f64(
            bytes as f64 / bytes_per_second as f64,
        ))
        .await;
    }
}

impl Default for TransferRateLimiter {
    fn default() -> Self {
        Self::new(None)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SftpOperation {
    ReadDirectory,
    ReadDirectoryTree,
    ReadFile,
    WriteFile,
    CreateFile,
    CreateDirectory,
    DeletePaths,
    UploadFile,
    DownloadFile,
    CancelTransfer,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SftpTransferDirection {
    Upload,
    Download,
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
pub struct RemoteDirectoryTree {
    pub root: String,
    pub directories: Vec<String>,
    pub files: Vec<RemoteFileEntry>,
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
    ReadDirectoryTree {
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
    CreateFile {
        request_id: u64,
        path: String,
    },
    CreateDirectories {
        request_id: u64,
        paths: Vec<String>,
    },
    DeletePaths {
        request_id: u64,
        paths: Vec<String>,
    },
    UploadFile {
        transfer_id: u64,
        local_path: PathBuf,
        remote_path: String,
        overwrite: bool,
    },
    DownloadFile {
        transfer_id: u64,
        remote_path: String,
        local_path: PathBuf,
        overwrite: bool,
    },
    CancelTransfer {
        transfer_id: u64,
    },
}

pub(crate) struct SftpWorkerHandle {
    command_tx: mpsc::UnboundedSender<SftpCommand>,
}

impl SftpWorkerHandle {
    pub(crate) fn spawn_with_limiter(
        session: SftpSession,
        events: mpsc::Sender<ConnectionEvent>,
        rate_limiter: Arc<TransferRateLimiter>,
    ) -> Self {
        let (command_tx, mut commands) = mpsc::unbounded_channel();

        tokio::spawn(async move {
            let session = Arc::new(session);
            let transfer_cancellations =
                Arc::new(Mutex::new(HashMap::<u64, Arc<AtomicBool>>::new()));
            let mut transfer_tasks = JoinSet::new();

            while let Some(command) = commands.recv().await {
                while transfer_tasks.try_join_next().is_some() {}
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
                    SftpCommand::ReadDirectoryTree { request_id, path } => {
                        let event = match read_directory_tree(&session, path.clone()).await {
                            Ok(tree) => ConnectionEvent::DirectoryTreeRead { request_id, tree },
                            Err(error) => ConnectionEvent::SftpFailed {
                                request_id,
                                path,
                                operation: SftpOperation::ReadDirectoryTree,
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
                    SftpCommand::CreateFile { request_id, path } => {
                        let event = match create_file(&session, path.clone()).await {
                            Ok(path) => ConnectionEvent::PathCreated {
                                request_id,
                                path,
                                kind: RemoteFileKind::File,
                            },
                            Err(error) => ConnectionEvent::SftpFailed {
                                request_id,
                                path,
                                operation: SftpOperation::CreateFile,
                                error,
                            },
                        };

                        if events.send(event).await.is_err() {
                            break;
                        }
                    }
                    SftpCommand::CreateDirectories { request_id, paths } => {
                        let error_path = paths.first().cloned().unwrap_or_default();
                        let event = match create_directories(&session, paths.clone()).await {
                            Ok(()) => ConnectionEvent::DirectoriesCreated { request_id, paths },
                            Err(error) => ConnectionEvent::SftpFailed {
                                request_id,
                                path: error_path,
                                operation: SftpOperation::CreateDirectory,
                                error,
                            },
                        };

                        if events.send(event).await.is_err() {
                            break;
                        }
                    }
                    SftpCommand::DeletePaths { request_id, paths } => {
                        let error_path = paths.first().cloned().unwrap_or_default();
                        let event = match delete_paths(&session, &paths).await {
                            Ok(()) => ConnectionEvent::PathsDeleted { request_id, paths },
                            Err(error) => ConnectionEvent::SftpFailed {
                                request_id,
                                path: error_path,
                                operation: SftpOperation::DeletePaths,
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
                    SftpCommand::UploadFile {
                        transfer_id,
                        local_path,
                        remote_path,
                        overwrite,
                    } => {
                        let cancellation = Arc::new(AtomicBool::new(false));
                        transfer_cancellations
                            .lock()
                            .expect("SFTP cancellation map should not be poisoned")
                            .insert(transfer_id, cancellation.clone());
                        let session = session.clone();
                        let events = events.clone();
                        let transfer_cancellations = transfer_cancellations.clone();
                        let rate_limiter = rate_limiter.clone();
                        transfer_tasks.spawn(async move {
                            let context = TransferContext::new(
                                transfer_id,
                                cancellation,
                                events.clone(),
                                rate_limiter,
                            );
                            let result = upload_file(
                                &session,
                                &local_path,
                                remote_path.clone(),
                                overwrite,
                                &context,
                            )
                            .await;
                            let event = transfer_result_event(
                                transfer_id,
                                remote_path,
                                SftpTransferDirection::Upload,
                                result,
                            );
                            let _ = events.send(event).await;
                            transfer_cancellations
                                .lock()
                                .expect("SFTP cancellation map should not be poisoned")
                                .remove(&transfer_id);
                        });
                    }
                    SftpCommand::DownloadFile {
                        transfer_id,
                        remote_path,
                        local_path,
                        overwrite,
                    } => {
                        let cancellation = Arc::new(AtomicBool::new(false));
                        transfer_cancellations
                            .lock()
                            .expect("SFTP cancellation map should not be poisoned")
                            .insert(transfer_id, cancellation.clone());
                        let session = session.clone();
                        let events = events.clone();
                        let transfer_cancellations = transfer_cancellations.clone();
                        let rate_limiter = rate_limiter.clone();
                        transfer_tasks.spawn(async move {
                            let context = TransferContext::new(
                                transfer_id,
                                cancellation,
                                events.clone(),
                                rate_limiter,
                            );
                            let result = download_file(
                                session,
                                remote_path.clone(),
                                &local_path,
                                overwrite,
                                &context,
                            )
                            .await;
                            let event = transfer_result_event(
                                transfer_id,
                                remote_path,
                                SftpTransferDirection::Download,
                                result,
                            );
                            let _ = events.send(event).await;
                            transfer_cancellations
                                .lock()
                                .expect("SFTP cancellation map should not be poisoned")
                                .remove(&transfer_id);
                        });
                    }
                    SftpCommand::CancelTransfer { transfer_id } => {
                        if let Some(cancellation) = transfer_cancellations
                            .lock()
                            .expect("SFTP cancellation map should not be poisoned")
                            .get(&transfer_id)
                        {
                            cancellation.store(true, Ordering::Release);
                        }
                    }
                }
            }

            {
                let transfer_cancellations = transfer_cancellations
                    .lock()
                    .expect("SFTP cancellation map should not be poisoned");
                for cancellation in transfer_cancellations.values() {
                    cancellation.store(true, Ordering::Release);
                }
            }
            if timeout(Duration::from_secs(2), async {
                while transfer_tasks.join_next().await.is_some() {}
            })
            .await
            .is_err()
            {
                transfer_tasks.abort_all();
                while transfer_tasks.join_next().await.is_some() {}
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

    pub(crate) fn read_directory_tree(
        &self,
        request_id: u64,
        path: String,
    ) -> Result<(), SshError> {
        self.command_tx
            .send(SftpCommand::ReadDirectoryTree { request_id, path })
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

    pub(crate) fn create_file(&self, request_id: u64, path: String) -> Result<(), SshError> {
        self.command_tx
            .send(SftpCommand::CreateFile { request_id, path })
            .map_err(|_| SshError::new(SshErrorKind::Sftp, "SFTP file worker is not running"))
    }

    pub(crate) fn create_directories(
        &self,
        request_id: u64,
        paths: Vec<String>,
    ) -> Result<(), SshError> {
        self.command_tx
            .send(SftpCommand::CreateDirectories { request_id, paths })
            .map_err(|_| SshError::new(SshErrorKind::Sftp, "SFTP directory worker is not running"))
    }

    pub(crate) fn delete_paths(&self, request_id: u64, paths: Vec<String>) -> Result<(), SshError> {
        self.command_tx
            .send(SftpCommand::DeletePaths { request_id, paths })
            .map_err(|_| SshError::new(SshErrorKind::Sftp, "SFTP delete worker is not running"))
    }

    pub(crate) fn upload_file(
        &self,
        transfer_id: u64,
        local_path: PathBuf,
        remote_path: String,
        overwrite: bool,
    ) -> Result<(), SshError> {
        self.command_tx
            .send(SftpCommand::UploadFile {
                transfer_id,
                local_path,
                remote_path,
                overwrite,
            })
            .map_err(|_| SshError::new(SshErrorKind::Sftp, "SFTP transfer worker is not running"))
    }

    pub(crate) fn download_file(
        &self,
        transfer_id: u64,
        remote_path: String,
        local_path: PathBuf,
        overwrite: bool,
    ) -> Result<(), SshError> {
        self.command_tx
            .send(SftpCommand::DownloadFile {
                transfer_id,
                remote_path,
                local_path,
                overwrite,
            })
            .map_err(|_| SshError::new(SshErrorKind::Sftp, "SFTP transfer worker is not running"))
    }

    pub(crate) fn cancel_transfer(&self, transfer_id: u64) -> Result<(), SshError> {
        self.command_tx
            .send(SftpCommand::CancelTransfer { transfer_id })
            .map_err(|_| SshError::new(SshErrorKind::Sftp, "SFTP transfer worker is not running"))
    }
}

pub(crate) const TRANSFER_CHUNK_BYTES: usize = 128 * 1024;
const DOWNLOAD_PIPELINE_STREAMS: usize = 4;
const MIN_DOWNLOAD_SEGMENT_BYTES: u64 = 1024 * 1024;
static NEXT_TRANSFER_TEMPORARY_ID: AtomicU64 = AtomicU64::new(1);

pub(crate) enum TransferResult {
    Completed(u64),
    Conflict,
    Cancelled,
}

#[derive(Clone)]
pub(crate) struct TransferContext {
    transfer_id: u64,
    cancellation: Arc<AtomicBool>,
    events: mpsc::Sender<ConnectionEvent>,
    rate_limiter: Arc<TransferRateLimiter>,
}

impl TransferContext {
    pub(crate) fn new(
        transfer_id: u64,
        cancellation: Arc<AtomicBool>,
        events: mpsc::Sender<ConnectionEvent>,
        rate_limiter: Arc<TransferRateLimiter>,
    ) -> Self {
        Self {
            transfer_id,
            cancellation,
            events,
            rate_limiter,
        }
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancellation.load(Ordering::Acquire)
    }

    pub(crate) const fn transfer_id(&self) -> u64 {
        self.transfer_id
    }

    pub(crate) async fn acquire_rate_budget(&self, bytes: usize) {
        self.rate_limiter.acquire(bytes).await;
    }

    pub(crate) async fn report_progress(
        &self,
        transferred: u64,
        total: Option<u64>,
    ) -> Result<(), SshError> {
        self.events
            .send(ConnectionEvent::TransferProgress {
                transfer_id: self.transfer_id,
                transferred,
                total,
            })
            .await
            .map_err(|_| {
                SshError::new(
                    SshErrorKind::InvalidState,
                    "SSH connection event receiver is not running",
                )
            })
    }
}

pub(crate) fn transfer_result_event(
    transfer_id: u64,
    path: String,
    direction: SftpTransferDirection,
    result: Result<TransferResult, SshError>,
) -> ConnectionEvent {
    match result {
        Ok(TransferResult::Completed(bytes)) => ConnectionEvent::TransferCompleted {
            transfer_id,
            direction,
            path,
            bytes,
        },
        Ok(TransferResult::Conflict) => ConnectionEvent::TransferConflict {
            transfer_id,
            direction,
            path,
        },
        Ok(TransferResult::Cancelled) => ConnectionEvent::TransferCancelled { transfer_id },
        Err(error) => ConnectionEvent::SftpFailed {
            request_id: transfer_id,
            path,
            operation: match direction {
                SftpTransferDirection::Upload => SftpOperation::UploadFile,
                SftpTransferDirection::Download => SftpOperation::DownloadFile,
            },
            error,
        },
    }
}

async fn upload_file(
    session: &SftpSession,
    local_path: &Path,
    remote_path: String,
    overwrite: bool,
    context: &TransferContext,
) -> Result<TransferResult, SshError> {
    let metadata = fs::metadata(local_path)
        .await
        .map_err(|error| transfer_io_error("reading local file metadata", error))?;
    if !metadata.is_file() {
        return Err(SshError::new(
            SshErrorKind::Sftp,
            "Only regular files can be uploaded",
        ));
    }
    let total = metadata.len();

    if session
        .try_exists(remote_path.clone())
        .await
        .map_err(SshError::from)?
        && !overwrite
    {
        return Ok(TransferResult::Conflict);
    }

    if context.is_cancelled() {
        return Ok(TransferResult::Cancelled);
    }

    let temporary_path = remote_transfer_temporary_path(
        &remote_path,
        &transfer_temporary_suffix(context.transfer_id),
    );
    if session
        .try_exists(temporary_path.clone())
        .await
        .map_err(SshError::from)?
    {
        session
            .remove_file(temporary_path.clone())
            .await
            .map_err(SshError::from)?;
    }

    let copy_result = copy_upload(session, local_path, &temporary_path, total, context).await;
    let transferred = match copy_result {
        Ok(TransferResult::Completed(bytes)) => bytes,
        Ok(TransferResult::Cancelled) => {
            let _ = session.remove_file(temporary_path).await;
            return Ok(TransferResult::Cancelled);
        }
        Ok(TransferResult::Conflict) => unreachable!("copy cannot report a conflict"),
        Err(error) => {
            let _ = session.remove_file(temporary_path).await;
            return Err(error);
        }
    };

    if context.is_cancelled() {
        let _ = session.remove_file(temporary_path).await;
        return Ok(TransferResult::Cancelled);
    }

    let install_result = async {
        if overwrite
            && session
                .try_exists(remote_path.clone())
                .await
                .map_err(SshError::from)?
        {
            session
                .remove_file(remote_path.clone())
                .await
                .map_err(SshError::from)?;
        }
        session
            .rename(temporary_path.clone(), remote_path)
            .await
            .map_err(SshError::from)
    }
    .await;
    if let Err(error) = install_result {
        let _ = session.remove_file(temporary_path).await;
        return Err(error);
    }

    Ok(TransferResult::Completed(transferred))
}

async fn copy_upload(
    session: &SftpSession,
    local_path: &Path,
    temporary_path: &str,
    total: u64,
    context: &TransferContext,
) -> Result<TransferResult, SshError> {
    let mut local_file = fs::File::open(local_path)
        .await
        .map_err(|error| transfer_io_error("opening local file", error))?;
    let mut remote_file = session
        .create(temporary_path.to_owned())
        .await
        .map_err(SshError::from)?;
    let mut buffer = vec![0; TRANSFER_CHUNK_BYTES];
    let mut transferred = 0_u64;

    loop {
        if context.is_cancelled() {
            return Ok(TransferResult::Cancelled);
        }
        let read = local_file
            .read(&mut buffer)
            .await
            .map_err(|error| transfer_io_error("reading local file", error))?;
        if read == 0 {
            break;
        }
        context.acquire_rate_budget(read).await;
        if context.is_cancelled() {
            return Ok(TransferResult::Cancelled);
        }
        remote_file
            .write_all(&buffer[..read])
            .await
            .map_err(|error| transfer_io_error("writing remote file", error))?;
        transferred += read as u64;
        context.report_progress(transferred, Some(total)).await?;
    }

    remote_file.sync_all().await.map_err(SshError::from)?;
    remote_file
        .shutdown()
        .await
        .map_err(|error| transfer_io_error("closing remote file", error))?;
    Ok(TransferResult::Completed(transferred))
}

async fn download_file(
    session: Arc<SftpSession>,
    remote_path: String,
    local_path: &Path,
    overwrite: bool,
    context: &TransferContext,
) -> Result<TransferResult, SshError> {
    if fs::try_exists(local_path)
        .await
        .map_err(|error| transfer_io_error("checking local destination", error))?
        && !overwrite
    {
        return Ok(TransferResult::Conflict);
    }

    if context.is_cancelled() {
        return Ok(TransferResult::Cancelled);
    }

    let remote_path = session
        .canonicalize(remote_path)
        .await
        .map_err(SshError::from)?;
    let metadata = session
        .metadata(remote_path.clone())
        .await
        .map_err(SshError::from)?;
    let total = metadata.size;
    let temporary_path =
        local_transfer_temporary_path(local_path, &transfer_temporary_suffix(context.transfer_id));
    if fs::try_exists(&temporary_path)
        .await
        .map_err(|error| transfer_io_error("checking temporary download", error))?
    {
        fs::remove_file(&temporary_path)
            .await
            .map_err(|error| transfer_io_error("removing stale temporary download", error))?;
    }

    let copy_result = copy_download(
        session,
        remote_path,
        temporary_path.clone(),
        total,
        context.clone(),
    )
    .await;
    let transferred = match copy_result {
        Ok(TransferResult::Completed(bytes)) => bytes,
        Ok(TransferResult::Cancelled) => {
            let _ = fs::remove_file(&temporary_path).await;
            return Ok(TransferResult::Cancelled);
        }
        Ok(TransferResult::Conflict) => unreachable!("copy cannot report a conflict"),
        Err(error) => {
            let _ = fs::remove_file(&temporary_path).await;
            return Err(error);
        }
    };

    if context.is_cancelled() {
        let _ = fs::remove_file(&temporary_path).await;
        return Ok(TransferResult::Cancelled);
    }

    let install_result = async {
        if overwrite
            && fs::try_exists(local_path)
                .await
                .map_err(|error| transfer_io_error("checking local destination", error))?
        {
            fs::remove_file(local_path)
                .await
                .map_err(|error| transfer_io_error("replacing local destination", error))?;
        }
        fs::rename(&temporary_path, local_path)
            .await
            .map_err(|error| transfer_io_error("installing downloaded file", error))
    }
    .await;
    if let Err(error) = install_result {
        let _ = fs::remove_file(&temporary_path).await;
        return Err(error);
    }

    Ok(TransferResult::Completed(transferred))
}

async fn copy_download(
    session: Arc<SftpSession>,
    remote_path: String,
    temporary_path: PathBuf,
    total: Option<u64>,
    context: TransferContext,
) -> Result<TransferResult, SshError> {
    let ranges = total.map(download_segment_ranges).unwrap_or_default();
    if ranges.len() > 1 {
        return copy_download_parallel(
            session,
            remote_path,
            temporary_path,
            total.expect("parallel download ranges require a known size"),
            ranges,
            context,
        )
        .await;
    }

    copy_download_sequential(&session, &remote_path, &temporary_path, total, &context).await
}

async fn copy_download_sequential(
    session: &SftpSession,
    remote_path: &str,
    temporary_path: &Path,
    total: Option<u64>,
    context: &TransferContext,
) -> Result<TransferResult, SshError> {
    let mut remote_file = session
        .open(remote_path.to_owned())
        .await
        .map_err(SshError::from)?;
    let mut local_file = fs::File::create(temporary_path)
        .await
        .map_err(|error| transfer_io_error("creating temporary download", error))?;
    let mut buffer = vec![0; TRANSFER_CHUNK_BYTES];
    let mut transferred = 0_u64;

    loop {
        if context.is_cancelled() {
            return Ok(TransferResult::Cancelled);
        }
        let read = remote_file
            .read(&mut buffer)
            .await
            .map_err(|error| transfer_io_error("reading remote file", error))?;
        if read == 0 {
            break;
        }
        context.acquire_rate_budget(read).await;
        if context.is_cancelled() {
            return Ok(TransferResult::Cancelled);
        }
        local_file
            .write_all(&buffer[..read])
            .await
            .map_err(|error| transfer_io_error("writing temporary download", error))?;
        transferred += read as u64;
        context.report_progress(transferred, total).await?;
    }

    local_file
        .sync_all()
        .await
        .map_err(|error| transfer_io_error("syncing downloaded file", error))?;
    Ok(TransferResult::Completed(transferred))
}

async fn copy_download_parallel(
    session: Arc<SftpSession>,
    remote_path: String,
    temporary_path: PathBuf,
    total: u64,
    ranges: Vec<Range<u64>>,
    context: TransferContext,
) -> Result<TransferResult, SshError> {
    let local_file = fs::File::create(&temporary_path)
        .await
        .map_err(|error| transfer_io_error("creating temporary download", error))?;
    local_file
        .set_len(total)
        .await
        .map_err(|error| transfer_io_error("preallocating temporary download", error))?;
    drop(local_file);

    let transferred = Arc::new(AtomicU64::new(0));
    let mut segments = JoinSet::new();
    for range in ranges {
        segments.spawn(copy_download_segment(
            session.clone(),
            remote_path.clone(),
            temporary_path.clone(),
            range,
            total,
            context.clone(),
            transferred.clone(),
        ));
    }

    let mut completed_bytes = 0_u64;
    while let Some(result) = segments.join_next().await {
        match result {
            Ok(Ok(TransferResult::Completed(bytes))) => {
                completed_bytes += bytes;
            }
            Ok(Ok(TransferResult::Cancelled)) => {
                segments.abort_all();
                while segments.join_next().await.is_some() {}
                return Ok(TransferResult::Cancelled);
            }
            Ok(Ok(TransferResult::Conflict)) => {
                unreachable!("download segments cannot report a conflict");
            }
            Ok(Err(error)) => {
                segments.abort_all();
                while segments.join_next().await.is_some() {}
                return Err(error);
            }
            Err(error) => {
                segments.abort_all();
                while segments.join_next().await.is_some() {}
                return Err(SshError::new(
                    SshErrorKind::Sftp,
                    format!("parallel download task failed: {error}"),
                ));
            }
        }
    }

    let local_file = fs::OpenOptions::new()
        .write(true)
        .open(&temporary_path)
        .await
        .map_err(|error| transfer_io_error("opening completed download", error))?;
    local_file
        .sync_all()
        .await
        .map_err(|error| transfer_io_error("syncing downloaded file", error))?;
    Ok(TransferResult::Completed(completed_bytes))
}

async fn copy_download_segment(
    session: Arc<SftpSession>,
    remote_path: String,
    temporary_path: PathBuf,
    range: Range<u64>,
    total: u64,
    context: TransferContext,
    transferred: Arc<AtomicU64>,
) -> Result<TransferResult, SshError> {
    let mut remote_file = session.open(remote_path).await.map_err(SshError::from)?;
    remote_file
        .seek(std::io::SeekFrom::Start(range.start))
        .await
        .map_err(|error| transfer_io_error("seeking remote file", error))?;
    let mut local_file = fs::OpenOptions::new()
        .write(true)
        .open(temporary_path)
        .await
        .map_err(|error| transfer_io_error("opening temporary download segment", error))?;
    local_file
        .seek(std::io::SeekFrom::Start(range.start))
        .await
        .map_err(|error| transfer_io_error("seeking temporary download segment", error))?;

    let mut remaining = range.end - range.start;
    let mut segment_bytes = 0_u64;
    let mut buffer = vec![0; TRANSFER_CHUNK_BYTES];
    while remaining > 0 {
        if context.is_cancelled() {
            return Ok(TransferResult::Cancelled);
        }
        let requested = usize::try_from(remaining.min(TRANSFER_CHUNK_BYTES as u64))
            .expect("download segment chunk fits usize");
        let read = remote_file
            .read(&mut buffer[..requested])
            .await
            .map_err(|error| transfer_io_error("reading remote download segment", error))?;
        if read == 0 {
            return Err(SshError::new(
                SshErrorKind::Sftp,
                "remote file ended before the download segment completed",
            ));
        }
        context.acquire_rate_budget(read).await;
        if context.is_cancelled() {
            return Ok(TransferResult::Cancelled);
        }
        local_file
            .write_all(&buffer[..read])
            .await
            .map_err(|error| transfer_io_error("writing temporary download segment", error))?;
        remaining -= read as u64;
        segment_bytes += read as u64;
        let aggregate = transferred.fetch_add(read as u64, Ordering::AcqRel) + read as u64;
        context.report_progress(aggregate, Some(total)).await?;
    }
    local_file
        .flush()
        .await
        .map_err(|error| transfer_io_error("flushing temporary download segment", error))?;
    remote_file
        .shutdown()
        .await
        .map_err(|error| transfer_io_error("closing remote download segment", error))?;
    Ok(TransferResult::Completed(segment_bytes))
}

fn download_segment_ranges(total: u64) -> Vec<Range<u64>> {
    if total == 0 {
        return Vec::new();
    }
    let stream_count = usize::try_from(total.div_ceil(MIN_DOWNLOAD_SEGMENT_BYTES))
        .unwrap_or(usize::MAX)
        .clamp(1, DOWNLOAD_PIPELINE_STREAMS);
    let segment_size = total.div_ceil(stream_count as u64);
    (0..stream_count)
        .map(|index| {
            let start = index as u64 * segment_size;
            start..(start + segment_size).min(total)
        })
        .filter(|range| range.start < range.end)
        .collect()
}

pub(crate) fn transfer_temporary_suffix(transfer_id: u64) -> String {
    let sequence = NEXT_TRANSFER_TEMPORARY_ID.fetch_add(1, Ordering::Relaxed);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!(
        "{transfer_id:x}-{:x}-{timestamp:x}-{sequence:x}",
        std::process::id()
    )
}

pub(crate) fn remote_transfer_temporary_path(path: &str, suffix: &str) -> String {
    format!("{path}.remcmd-{suffix}.part")
}

fn local_transfer_temporary_path(path: &Path, suffix: &str) -> PathBuf {
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_else(|| "download".into());
    path.with_file_name(format!(".{file_name}.remcmd-{suffix}.part"))
}

pub(crate) fn transfer_io_error(action: &str, error: std::io::Error) -> SshError {
    SshError::new(SshErrorKind::Sftp, format!("{action}: {error}"))
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

async fn read_directory_tree(
    session: &SftpSession,
    path: String,
) -> Result<RemoteDirectoryTree, SshError> {
    let root = session.canonicalize(path).await.map_err(SshError::from)?;
    let mut pending = vec![root.clone()];
    let mut directories = Vec::new();
    let mut files = Vec::new();

    while let Some(directory) = pending.pop() {
        let entries = session.read_dir(directory).await.map_err(SshError::from)?;
        for entry in entries {
            let metadata = entry.metadata();
            match remote_file_kind(entry.file_type()) {
                RemoteFileKind::Directory => {
                    let path = entry.path();
                    directories.push(path.clone());
                    pending.push(path);
                }
                RemoteFileKind::File | RemoteFileKind::Other if metadata.size.is_some() => {
                    files.push(RemoteFileEntry {
                        name: entry.file_name(),
                        path: entry.path(),
                        kind: RemoteFileKind::File,
                        size: metadata.size,
                        modified: metadata.mtime,
                    });
                }
                RemoteFileKind::File | RemoteFileKind::Symlink | RemoteFileKind::Other => {}
            }
        }
    }
    directories.sort_by(|left, right| {
        remote_path_depth(left)
            .cmp(&remote_path_depth(right))
            .then_with(|| left.cmp(right))
    });
    files.sort_by(|left, right| left.path.cmp(&right.path));

    Ok(RemoteDirectoryTree {
        root,
        directories,
        files,
    })
}

async fn create_file(session: &SftpSession, path: String) -> Result<String, SshError> {
    let mut file = session
        .open_with_flags(
            path.clone(),
            OpenFlags::CREATE | OpenFlags::EXCLUDE | OpenFlags::WRITE,
        )
        .await
        .map_err(SshError::from)?;
    file.sync_all().await.map_err(SshError::from)?;
    file.shutdown()
        .await
        .map_err(|error| transfer_io_error("closing new remote file", error))?;
    session.canonicalize(path).await.map_err(SshError::from)
}

async fn create_directories(session: &SftpSession, mut paths: Vec<String>) -> Result<(), SshError> {
    paths.sort_by(|left, right| {
        remote_path_depth(left)
            .cmp(&remote_path_depth(right))
            .then_with(|| left.cmp(right))
    });
    paths.dedup();

    for path in paths {
        if session
            .try_exists(path.clone())
            .await
            .map_err(SshError::from)?
        {
            let metadata = session
                .symlink_metadata(path)
                .await
                .map_err(SshError::from)?;
            if !metadata.file_type().is_dir() {
                return Err(SshError::new(
                    SshErrorKind::Sftp,
                    "Cannot create a directory over an existing remote file",
                ));
            }
            continue;
        }
        session.create_dir(path).await.map_err(SshError::from)?;
    }
    Ok(())
}

async fn delete_paths(session: &SftpSession, paths: &[String]) -> Result<(), SshError> {
    enum DeleteStep {
        Inspect(String),
        RemoveDirectory(String),
    }

    for requested_path in paths {
        let path = session
            .canonicalize(requested_path.clone())
            .await
            .map_err(SshError::from)?;
        if path == "/" {
            return Err(SshError::new(
                SshErrorKind::Sftp,
                "Refusing to delete the remote root directory",
            ));
        }

        let mut pending = vec![DeleteStep::Inspect(path)];
        while let Some(step) = pending.pop() {
            match step {
                DeleteStep::Inspect(path) => {
                    let metadata =
                        session
                            .symlink_metadata(path.clone())
                            .await
                            .map_err(|error| {
                                SshError::new(
                                    SshErrorKind::Sftp,
                                    format!("reading metadata for {path}: {error}"),
                                )
                            })?;
                    if metadata.file_type().is_dir() {
                        let entries = session.read_dir(path.clone()).await.map_err(|error| {
                            SshError::new(
                                SshErrorKind::Sftp,
                                format!("reading directory {path}: {error}"),
                            )
                        })?;
                        pending.push(DeleteStep::RemoveDirectory(path));
                        pending.extend(
                            entries
                                .map(|entry| DeleteStep::Inspect(entry.path()))
                                .collect::<Vec<_>>()
                                .into_iter()
                                .rev(),
                        );
                    } else {
                        session.remove_file(path.clone()).await.map_err(|error| {
                            SshError::new(
                                SshErrorKind::Sftp,
                                format!("deleting file {path}: {error}"),
                            )
                        })?;
                    }
                }
                DeleteStep::RemoveDirectory(path) => {
                    session.remove_dir(path.clone()).await.map_err(|error| {
                        SshError::new(
                            SshErrorKind::Sftp,
                            format!("deleting directory {path}: {error}"),
                        )
                    })?;
                }
            }
        }
    }
    Ok(())
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

fn remote_path_depth(path: &str) -> usize {
    path.split('/')
        .filter(|component| !component.is_empty())
        .count()
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
        collections::{HashMap, HashSet},
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

    #[tokio::test]
    async fn rate_limiter_shares_one_budget_between_concurrent_transfers() {
        let rate_limiter = Arc::new(TransferRateLimiter::new(Some(1024 * 1024)));
        let started = tokio::time::Instant::now();

        tokio::join!(
            rate_limiter.acquire(64 * 1024),
            rate_limiter.acquire(64 * 1024)
        );

        assert!(started.elapsed() >= Duration::from_millis(120));
        rate_limiter.set_bytes_per_second(None);
        timeout(Duration::from_millis(20), rate_limiter.acquire(64 * 1024))
            .await
            .expect("disabling the rate limit should take effect immediately");
    }

    struct TestSftpServer {
        directory_reads: HashSet<String>,
        directories: Arc<Mutex<HashSet<String>>>,
        files: Arc<Mutex<HashMap<String, Vec<u8>>>>,
        read_offsets: Arc<Mutex<Vec<u64>>>,
    }

    impl Default for TestSftpServer {
        fn default() -> Self {
            let files =
                HashMap::from([("/home/test/notes.txt".into(), b"original contents".to_vec())]);
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
        let worker = SftpWorkerHandle::spawn_with_limiter(
            session,
            event_tx,
            Arc::new(TransferRateLimiter::default()),
        );

        worker
            .upload_file(48, local_path, "/home/test/worker-drop.bin".into(), false)
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
}
