use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use russh_sftp::client::SftpSession;
use tokio::{
    sync::mpsc,
    task::JoinSet,
    time::{Duration, timeout},
};

use crate::{
    ConnectionEvent, RemoteFileKind, SftpOperation, SftpTransferDirection, SshError, SshErrorKind,
    SshTransport, TransferRateLimiter, scp, sftp,
    transfer::{TransferContext, TransferResult, transfer_result_event},
};

/// Remote-file requests shared by the connection and protocol workers.
#[derive(Debug, PartialEq, Eq)]
pub enum FileCommand {
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
    Transfer {
        transfer_id: u64,
        direction: SftpTransferDirection,
        local_path: PathBuf,
        remote_path: String,
        overwrite: bool,
    },
    CancelTransfer {
        transfer_id: u64,
    },
}

impl FileCommand {
    fn request(&self) -> FileRequest {
        let (request_id, path, operation) = match self {
            Self::ReadDirectory { request_id, path } => {
                (*request_id, path.as_str(), SftpOperation::ReadDirectory)
            }
            Self::ReadDirectoryTree { request_id, path } => {
                (*request_id, path.as_str(), SftpOperation::ReadDirectoryTree)
            }
            Self::ReadFile { request_id, path } => {
                (*request_id, path.as_str(), SftpOperation::ReadFile)
            }
            Self::WriteFile {
                request_id, path, ..
            } => (*request_id, path.as_str(), SftpOperation::WriteFile),
            Self::CreateFile { request_id, path } => {
                (*request_id, path.as_str(), SftpOperation::CreateFile)
            }
            Self::CreateDirectories { request_id, paths } => (
                *request_id,
                paths.first().map(String::as_str).unwrap_or_default(),
                SftpOperation::CreateDirectory,
            ),
            Self::DeletePaths { request_id, paths } => (
                *request_id,
                paths.first().map(String::as_str).unwrap_or_default(),
                SftpOperation::DeletePaths,
            ),
            Self::Transfer {
                transfer_id,
                direction,
                remote_path,
                ..
            } => (
                *transfer_id,
                remote_path.as_str(),
                match direction {
                    SftpTransferDirection::Upload => SftpOperation::UploadFile,
                    SftpTransferDirection::Download => SftpOperation::DownloadFile,
                },
            ),
            Self::CancelTransfer { transfer_id } => {
                (*transfer_id, "", SftpOperation::CancelTransfer)
            }
        };
        FileRequest {
            request_id,
            path: path.into(),
            operation,
        }
    }

    fn supports_scp(&self) -> bool {
        matches!(
            self,
            Self::CreateDirectories { .. }
                | Self::Transfer {
                    direction: SftpTransferDirection::Upload,
                    ..
                }
        )
    }
}

struct FileRequest {
    request_id: u64,
    path: String,
    operation: SftpOperation,
}

impl FileRequest {
    fn failure(self, error: SshError) -> ConnectionEvent {
        ConnectionEvent::SftpFailed {
            request_id: self.request_id,
            path: self.path,
            operation: self.operation,
            error,
        }
    }
}

#[derive(Clone)]
pub(crate) enum FileBackend {
    Sftp(Arc<SftpSession>),
    Scp(Arc<SshTransport>),
}

impl FileBackend {
    async fn execute(&self, command: FileCommand) -> Result<ConnectionEvent, SshError> {
        match (self, command) {
            (Self::Sftp(session), FileCommand::ReadDirectory { request_id, path }) => {
                let directory = sftp::read_directory(session, path).await?;
                Ok(ConnectionEvent::DirectoryRead {
                    request_id,
                    directory,
                })
            }
            (Self::Sftp(session), FileCommand::ReadDirectoryTree { request_id, path }) => {
                let tree = sftp::read_directory_tree(session, path).await?;
                Ok(ConnectionEvent::DirectoryTreeRead { request_id, tree })
            }
            (Self::Sftp(session), FileCommand::ReadFile { request_id, path }) => {
                let file = sftp::read_file(session, path).await?;
                Ok(ConnectionEvent::FileRead { request_id, file })
            }
            (
                Self::Sftp(session),
                FileCommand::WriteFile {
                    request_id,
                    path,
                    expected_contents,
                    contents,
                },
            ) => {
                let file = sftp::write_file(session, path, expected_contents, contents).await?;
                Ok(ConnectionEvent::FileWritten { request_id, file })
            }
            (Self::Sftp(session), FileCommand::CreateFile { request_id, path }) => {
                let path = sftp::create_file(session, path).await?;
                Ok(ConnectionEvent::PathCreated {
                    request_id,
                    path,
                    kind: RemoteFileKind::File,
                })
            }
            (backend, FileCommand::CreateDirectories { request_id, paths }) => {
                match backend {
                    Self::Sftp(session) => sftp::create_directories(session, paths.clone()).await?,
                    Self::Scp(transport) => scp::create_directories(transport, &paths).await?,
                }
                Ok(ConnectionEvent::DirectoriesCreated { request_id, paths })
            }
            (Self::Sftp(session), FileCommand::DeletePaths { request_id, paths }) => {
                sftp::delete_paths(session, &paths).await?;
                Ok(ConnectionEvent::PathsDeleted { request_id, paths })
            }
            _ => Err(SshError::new(
                SshErrorKind::Sftp,
                "This file operation requires SFTP",
            )),
        }
    }

    async fn transfer(
        &self,
        direction: SftpTransferDirection,
        local_path: &std::path::Path,
        remote_path: &str,
        overwrite: bool,
        context: &TransferContext,
    ) -> Result<TransferResult, SshError> {
        match (self, direction) {
            (Self::Sftp(session), SftpTransferDirection::Upload) => {
                sftp::upload_file(session, local_path, remote_path.into(), overwrite, context).await
            }
            (Self::Sftp(session), SftpTransferDirection::Download) => {
                sftp::download_file(
                    session.clone(),
                    remote_path.into(),
                    local_path,
                    overwrite,
                    context,
                )
                .await
            }
            (Self::Scp(transport), SftpTransferDirection::Upload) => {
                scp::upload_file(transport, local_path, remote_path, overwrite, context).await
            }
            (Self::Scp(_), SftpTransferDirection::Download) => Err(SshError::new(
                SshErrorKind::Sftp,
                "Downloading files requires SFTP",
            )),
        }
    }
}

pub(crate) struct FileWorkerHandle(mpsc::UnboundedSender<FileCommand>);

impl FileWorkerHandle {
    pub(crate) fn spawn(
        backend: FileBackend,
        events: mpsc::Sender<ConnectionEvent>,
        rate_limiter: Arc<TransferRateLimiter>,
    ) -> Self {
        let (command_tx, mut commands) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            let cancellations = Arc::new(Mutex::new(HashMap::<u64, Arc<AtomicBool>>::new()));
            let mut transfers = JoinSet::new();
            while let Some(command) = commands.recv().await {
                while transfers.try_join_next().is_some() {}
                match command {
                    FileCommand::Transfer {
                        transfer_id,
                        direction,
                        local_path,
                        remote_path,
                        overwrite,
                    } => {
                        let cancellation = Arc::new(AtomicBool::new(false));
                        cancellations
                            .lock()
                            .expect("transfer cancellation map")
                            .insert(transfer_id, cancellation.clone());
                        let backend = backend.clone();
                        let events = events.clone();
                        let cancellations = cancellations.clone();
                        let context = TransferContext::new(
                            transfer_id,
                            cancellation,
                            events.clone(),
                            rate_limiter.clone(),
                        );
                        transfers.spawn(async move {
                            let result = backend
                                .transfer(direction, &local_path, &remote_path, overwrite, &context)
                                .await;
                            let _ = events
                                .send(transfer_result_event(
                                    transfer_id,
                                    remote_path,
                                    direction,
                                    result,
                                ))
                                .await;
                            cancellations
                                .lock()
                                .expect("transfer cancellation map")
                                .remove(&transfer_id);
                        });
                    }
                    FileCommand::CancelTransfer { transfer_id } => {
                        if let Some(cancel) = cancellations
                            .lock()
                            .expect("transfer cancellation map")
                            .get(&transfer_id)
                        {
                            cancel.store(true, Ordering::Release);
                        }
                    }
                    command => {
                        let request = command.request();
                        let event = backend
                            .execute(command)
                            .await
                            .unwrap_or_else(|error| request.failure(error));
                        if events.send(event).await.is_err() {
                            break;
                        }
                    }
                }
            }
            for cancel in cancellations
                .lock()
                .expect("transfer cancellation map")
                .values()
            {
                cancel.store(true, Ordering::Release);
            }
            if timeout(Duration::from_secs(2), async {
                while transfers.join_next().await.is_some() {}
            })
            .await
            .is_err()
            {
                transfers.shutdown().await;
            }
            if let FileBackend::Sftp(session) = backend {
                let _ = session.close().await;
            }
        });
        Self(command_tx)
    }

    pub(crate) fn send(
        &self,
        command: FileCommand,
    ) -> Result<(), mpsc::error::SendError<FileCommand>> {
        self.0.send(command)
    }
}

/// Lazily opens file channels while retaining SCP transfers during SFTP recovery.
pub(crate) struct RemoteFiles {
    transport: Arc<SshTransport>,
    events: mpsc::Sender<ConnectionEvent>,
    rate_limiter: Arc<TransferRateLimiter>,
    sftp_worker: Option<FileWorkerHandle>,
    scp_worker: Option<FileWorkerHandle>,
    pub(crate) sftp_available: Option<bool>,
    pub(crate) scp_available: Option<bool>,
}

impl RemoteFiles {
    pub(crate) fn new(
        transport: Arc<SshTransport>,
        events: mpsc::Sender<ConnectionEvent>,
        rate_limiter: Arc<TransferRateLimiter>,
    ) -> Self {
        Self {
            transport,
            events,
            rate_limiter,
            sftp_worker: None,
            scp_worker: None,
            sftp_available: None,
            scp_available: None,
        }
    }

    pub(crate) async fn dispatch(&mut self, command: FileCommand, probe_pending: bool) -> bool {
        if let FileCommand::CancelTransfer { transfer_id } = command {
            let mut first_error = None;
            for worker in [&self.sftp_worker, &self.scp_worker].into_iter().flatten() {
                if let Err(error) = worker.send(FileCommand::CancelTransfer { transfer_id }) {
                    first_error.get_or_insert(error);
                }
            }
            return match first_error {
                Some(error) => self.report_send_error(error).await,
                None => true,
            };
        }
        match self.worker_for(&command, probe_pending).await {
            Ok(worker) => match worker.send(command) {
                Ok(()) => true,
                Err(error) => self.report_send_error(error).await,
            },
            Err(error) => self
                .events
                .send(command.request().failure(error))
                .await
                .is_ok(),
        }
    }

    async fn report_send_error(&self, error: mpsc::error::SendError<FileCommand>) -> bool {
        self.events
            .send(error.0.request().failure(SshError::new(
                SshErrorKind::Sftp,
                "Remote file worker is not running",
            )))
            .await
            .is_ok()
    }

    async fn worker_for(
        &mut self,
        command: &FileCommand,
        probe_pending: bool,
    ) -> Result<&FileWorkerHandle, SshError> {
        if matches!(command, FileCommand::ReadDirectory { .. }) && self.sftp_available != Some(true)
        {
            return Err(SshError::new(
                SshErrorKind::Sftp,
                if probe_pending {
                    "SFTP availability is still being checked"
                } else {
                    "SFTP is unavailable on this server"
                },
            ));
        }
        let fallback = command.supports_scp();
        if self.sftp_worker.is_none() && (!fallback || self.sftp_available == Some(true)) {
            match self.transport.open_sftp().await {
                Ok(session) => {
                    self.sftp_worker = Some(FileWorkerHandle::spawn(
                        FileBackend::Sftp(Arc::new(session)),
                        self.events.clone(),
                        self.rate_limiter.clone(),
                    ))
                }
                Err(error) if fallback => {
                    self.sftp_available = Some(false);
                    if self.scp_available != Some(true) {
                        return Err(error);
                    }
                    self.events
                        .send(ConnectionEvent::SftpAvailabilityChanged {
                            available: false,
                            scp_available: true,
                            message: Some(format!(
                                "Opening SFTP failed: {error}; using SCP upload fallback"
                            )),
                        })
                        .await
                        .map_err(|_| {
                            SshError::new(
                                SshErrorKind::InvalidState,
                                "SSH connection event receiver is not running",
                            )
                        })?;
                }
                Err(error) => return Err(error),
            }
        }
        if let Some(worker) = &self.sftp_worker {
            return Ok(worker);
        }
        if fallback && self.scp_available == Some(true) {
            return Ok(self.scp_worker.get_or_insert_with(|| {
                FileWorkerHandle::spawn(
                    FileBackend::Scp(self.transport.clone()),
                    self.events.clone(),
                    self.rate_limiter.clone(),
                )
            }));
        }
        Err(SshError::new(
            SshErrorKind::Sftp,
            if probe_pending {
                "File-transfer availability is still being checked"
            } else {
                "SFTP and SCP are unavailable on this server"
            },
        ))
    }
}
