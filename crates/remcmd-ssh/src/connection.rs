use std::{
    future::Future,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use remcmd_core::ConnectionProfile;
use tokio::{
    runtime::Handle,
    sync::mpsc,
    time::{Instant, sleep_until},
};

use crate::{
    AuthMethod, AuthMethodKind, ConnectionPlan, ConnectionStage, HostKeyInfo, PtySize,
    RemoteDirectory, RemoteDirectoryTree, RemoteFile, RemoteFileKind, SessionState, SftpOperation,
    SftpTransferDirection, ShellEvent, SshError, SshErrorKind, SshSession, SshShellWriter,
    SshTransport, TransferRateLimiter,
    host_key::HostKeyDecision,
    performance::{PerformanceMonitorHandle, ServerPerformanceSnapshot},
    sftp::SftpWorkerHandle,
    transport::TransportOpen,
};

const EVENT_CHANNEL_CAPACITY: usize = 256;
const SHELL_INTEGRATION_QUIET_PERIOD: std::time::Duration = std::time::Duration::from_millis(120);

/// Commands sent from the application to one running SSH session.
///
/// Connect is not a command because each worker is created for one connection
/// attempt with its profile and authentication data supplied at startup.
#[derive(Debug, PartialEq, Eq)]
pub enum ConnectionCommand {
    /// Sends raw keyboard or paste bytes to the remote shell.
    Input(Vec<u8>),

    /// Reports a new terminal size to the remote PTY.
    Resize(PtySize),

    /// Reads one remote directory through an SFTP subsystem channel.
    ReadDirectory { request_id: u64, path: String },

    /// Recursively lists regular files and directories below one remote path.
    ReadDirectoryTree { request_id: u64, path: String },

    /// Reads one remote file through an SFTP subsystem channel.
    ReadFile { request_id: u64, path: String },

    /// Replaces a remote file if its contents have not changed since it was read.
    WriteFile {
        request_id: u64,
        path: String,
        expected_contents: Vec<u8>,
        contents: Vec<u8>,
    },

    /// Creates one empty remote file without replacing an existing item.
    CreateFile { request_id: u64, path: String },

    /// Creates remote directories in parent-first order.
    CreateDirectories { request_id: u64, paths: Vec<String> },

    /// Recursively deletes remote files and directories.
    DeletePaths { request_id: u64, paths: Vec<String> },

    /// Copies one local file to a remote SFTP path.
    UploadFile {
        transfer_id: u64,
        local_path: PathBuf,
        remote_path: String,
        overwrite: bool,
    },

    /// Copies one remote SFTP file to a local path.
    DownloadFile {
        transfer_id: u64,
        remote_path: String,
        local_path: PathBuf,
        overwrite: bool,
    },

    /// Requests cancellation of an active SFTP transfer.
    CancelTransfer { transfer_id: u64 },

    /// Starts or stops periodic server performance sampling.
    SetPerformanceMonitoring(bool),

    /// Requests an orderly shell and transport shutdown.
    Disconnect,
}

/// Events sent from one SSH worker back to the application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionEvent {
    /// Reports a successful lifecycle transition.
    StateChanged(SessionState),

    /// Identifies the proxy, jump, or target currently being established.
    ConnectionStageChanged(ConnectionStage),

    /// Reports independent authentication success for one SSH step.
    AuthenticationSucceeded {
        stage: ConnectionStage,
        method: AuthMethodKind,
    },

    /// Pauses the SSH handshake until the user verifies an unknown server key.
    HostKeyVerificationRequired {
        stage: ConnectionStage,
        info: HostKeyInfo,
    },

    /// Confirms that the remote PTY accepted a terminal resize.
    Resized(PtySize),

    /// Carries output or lifecycle information from the remote shell.
    Shell(ShellEvent),

    /// Returns one canonical remote path and its directory entries.
    DirectoryRead {
        request_id: u64,
        directory: RemoteDirectory,
    },

    /// Returns one recursively enumerated remote directory tree.
    DirectoryTreeRead {
        request_id: u64,
        tree: RemoteDirectoryTree,
    },

    /// Returns one canonical remote path and its file contents.
    FileRead { request_id: u64, file: RemoteFile },

    /// Confirms that a remote file was replaced and returns its saved contents.
    FileWritten { request_id: u64, file: RemoteFile },

    /// Confirms that one new remote file was created.
    PathCreated {
        request_id: u64,
        path: String,
        kind: RemoteFileKind,
    },

    /// Confirms that the requested remote directories exist.
    DirectoriesCreated { request_id: u64, paths: Vec<String> },

    /// Confirms recursive deletion of the requested remote paths.
    PathsDeleted { request_id: u64, paths: Vec<String> },

    /// Reports incremental bytes copied by an SFTP transfer.
    TransferProgress {
        transfer_id: u64,
        transferred: u64,
        total: Option<u64>,
    },

    /// Pauses a transfer because its destination already exists.
    TransferConflict {
        transfer_id: u64,
        direction: SftpTransferDirection,
        path: String,
    },

    /// Confirms that a transfer installed its final destination.
    TransferCompleted {
        transfer_id: u64,
        direction: SftpTransferDirection,
        path: String,
        bytes: u64,
    },

    /// Confirms that a transfer stopped without replacing its destination.
    TransferCancelled { transfer_id: u64 },

    /// Reports an SFTP operation failure without failing the SSH shell.
    SftpFailed {
        request_id: u64,
        path: String,
        operation: SftpOperation,
        error: SshError,
    },

    /// Reports whether the server exposes an SFTP subsystem before the UI uses it.
    SftpAvailabilityChanged {
        available: bool,
        message: Option<String>,
    },

    /// Reports one sample from the independent server performance channel.
    PerformanceSnapshot(ServerPerformanceSnapshot),

    /// Reports a performance sampling failure without failing the SSH shell.
    PerformanceFailed(SshError),

    /// Reports an operational failure and implies SessionState::Failed.
    Failed(SshError),
}

/// Cloneable command handle retained by the application.
///
/// Cloning this handle only clones the channel sender. It does not duplicate
/// the SSH transport, shell, or queued command data.
#[derive(Clone)]
pub struct ConnectionHandle {
    command_tx: mpsc::UnboundedSender<ConnectionCommand>,
    host_key_decision_tx: mpsc::UnboundedSender<HostKeyDecision>,
    host_key_verification_pending: Arc<AtomicBool>,
}

impl ConnectionHandle {
    /// Sends raw input bytes to the running shell.
    pub fn send_input(&self, data: impl Into<Vec<u8>>) -> Result<(), SshError> {
        self.send(ConnectionCommand::Input(data.into()))
    }

    /// Requests a remote PTY resize.
    pub fn resize(&self, size: PtySize) -> Result<(), SshError> {
        self.send(ConnectionCommand::Resize(size))
    }

    /// Requests one remote directory listing through this SSH connection.
    pub fn read_directory(&self, request_id: u64, path: impl Into<String>) -> Result<(), SshError> {
        self.send(ConnectionCommand::ReadDirectory {
            request_id,
            path: path.into(),
        })
    }

    /// Requests a recursive remote directory listing through this SSH connection.
    pub fn read_directory_tree(
        &self,
        request_id: u64,
        path: impl Into<String>,
    ) -> Result<(), SshError> {
        self.send(ConnectionCommand::ReadDirectoryTree {
            request_id,
            path: path.into(),
        })
    }

    /// Requests one remote file through this SSH connection.
    pub fn read_file(&self, request_id: u64, path: impl Into<String>) -> Result<(), SshError> {
        self.send(ConnectionCommand::ReadFile {
            request_id,
            path: path.into(),
        })
    }

    /// Replaces one remote file if it still matches the supplied original contents.
    pub fn write_file(
        &self,
        request_id: u64,
        path: impl Into<String>,
        expected_contents: Vec<u8>,
        contents: Vec<u8>,
    ) -> Result<(), SshError> {
        self.send(ConnectionCommand::WriteFile {
            request_id,
            path: path.into(),
            expected_contents,
            contents,
        })
    }

    /// Creates one empty remote file without overwriting an existing item.
    pub fn create_file(&self, request_id: u64, path: impl Into<String>) -> Result<(), SshError> {
        self.send(ConnectionCommand::CreateFile {
            request_id,
            path: path.into(),
        })
    }

    /// Creates remote directories in parent-first order.
    pub fn create_directories(&self, request_id: u64, paths: Vec<String>) -> Result<(), SshError> {
        self.send(ConnectionCommand::CreateDirectories { request_id, paths })
    }

    /// Recursively deletes remote files and directories.
    pub fn delete_paths(&self, request_id: u64, paths: Vec<String>) -> Result<(), SshError> {
        self.send(ConnectionCommand::DeletePaths { request_id, paths })
    }

    /// Queues a local file upload through this SSH connection.
    pub fn upload_file(
        &self,
        transfer_id: u64,
        local_path: PathBuf,
        remote_path: impl Into<String>,
        overwrite: bool,
    ) -> Result<(), SshError> {
        self.send(ConnectionCommand::UploadFile {
            transfer_id,
            local_path,
            remote_path: remote_path.into(),
            overwrite,
        })
    }

    /// Queues a remote file download through this SSH connection.
    pub fn download_file(
        &self,
        transfer_id: u64,
        remote_path: impl Into<String>,
        local_path: PathBuf,
        overwrite: bool,
    ) -> Result<(), SshError> {
        self.send(ConnectionCommand::DownloadFile {
            transfer_id,
            remote_path: remote_path.into(),
            local_path,
            overwrite,
        })
    }

    /// Requests cancellation of an active SFTP transfer.
    pub fn cancel_transfer(&self, transfer_id: u64) -> Result<(), SshError> {
        self.send(ConnectionCommand::CancelTransfer { transfer_id })
    }

    /// Starts or stops periodic server performance sampling.
    pub fn set_performance_monitoring(&self, enabled: bool) -> Result<(), SshError> {
        self.send(ConnectionCommand::SetPerformanceMonitoring(enabled))
    }

    /// Requests an orderly disconnection.
    pub fn disconnect(&self) -> Result<(), SshError> {
        self.send(ConnectionCommand::Disconnect)
    }

    /// Trusts and records the unknown host key presented by this connection.
    pub fn trust_host_key(&self) -> Result<(), SshError> {
        self.send_host_key_decision(HostKeyDecision::Trust)
    }

    /// Rejects the unknown host key presented by this connection.
    pub fn reject_host_key(&self) -> Result<(), SshError> {
        self.send_host_key_decision(HostKeyDecision::Reject)
    }

    fn send(&self, command: ConnectionCommand) -> Result<(), SshError> {
        self.command_tx.send(command).map_err(|_| {
            SshError::new(
                SshErrorKind::InvalidState,
                "SSH connection task is not running",
            )
        })
    }

    fn send_host_key_decision(&self, decision: HostKeyDecision) -> Result<(), SshError> {
        self.host_key_verification_pending
            .compare_exchange(true, false, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| {
                SshError::new(
                    SshErrorKind::InvalidState,
                    "SSH host-key verification is not pending",
                )
            })?;

        self.host_key_decision_tx.send(decision).map_err(|_| {
            SshError::new(
                SshErrorKind::InvalidState,
                "SSH host-key verification is not pending",
            )
        })
    }
}

/// Owns the receiving side of one connection's event channel.
///
/// Only one consumer should process a session's ordered events, so this type
/// intentionally does not implement Clone.
pub struct ConnectionEventReceiver {
    event_rx: mpsc::Receiver<ConnectionEvent>,
}

impl ConnectionEventReceiver {
    /// Waits for the next event, returning None after the worker exits.
    pub async fn next_event(&mut self) -> Option<ConnectionEvent> {
        self.event_rx.recv().await
    }

    /// Returns one already-buffered event without waiting.
    pub fn try_next_event(&mut self) -> Option<ConnectionEvent> {
        self.event_rx.try_recv().ok()
    }
}

/// Owns the application-facing parts of one background SSH worker.
pub struct SshConnection {
    handle: ConnectionHandle,
    events: ConnectionEventReceiver,
}

impl SshConnection {
    /// Starts one SSH worker on the supplied Tokio runtime.
    pub fn spawn(
        runtime: &Handle,
        profile: ConnectionProfile,
        auth: AuthMethod,
        initial_size: PtySize,
    ) -> Self {
        Self::spawn_with_transfer_rate_limiter(
            runtime,
            profile,
            auth,
            initial_size,
            Arc::new(TransferRateLimiter::default()),
        )
    }

    pub fn spawn_with_transfer_rate_limiter(
        runtime: &Handle,
        profile: ConnectionProfile,
        auth: AuthMethod,
        initial_size: PtySize,
        transfer_rate_limiter: Arc<TransferRateLimiter>,
    ) -> Self {
        Self::spawn_plan_with_transfer_rate_limiter(
            runtime,
            ConnectionPlan::direct(profile, auth),
            initial_size,
            transfer_rate_limiter,
        )
    }

    pub fn spawn_plan(runtime: &Handle, plan: ConnectionPlan, initial_size: PtySize) -> Self {
        Self::spawn_plan_with_transfer_rate_limiter(
            runtime,
            plan,
            initial_size,
            Arc::new(TransferRateLimiter::default()),
        )
    }

    pub fn spawn_plan_with_transfer_rate_limiter(
        runtime: &Handle,
        plan: ConnectionPlan,
        initial_size: PtySize,
        transfer_rate_limiter: Arc<TransferRateLimiter>,
    ) -> Self {
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let (host_key_decision_tx, host_key_decision_rx) = mpsc::unbounded_channel();
        let host_key_verification_pending = Arc::new(AtomicBool::new(false));
        let (event_tx, event_rx) = mpsc::channel(EVENT_CHANNEL_CAPACITY);

        runtime.spawn(run_connection_plan(
            plan,
            initial_size,
            ConnectionWorkerContext {
                commands: command_rx,
                host_key_decisions: host_key_decision_rx,
                host_key_verification_pending: host_key_verification_pending.clone(),
                events: event_tx,
                transfer_rate_limiter,
            },
        ));

        Self {
            handle: ConnectionHandle {
                command_tx,
                host_key_decision_tx,
                host_key_verification_pending,
            },
            events: ConnectionEventReceiver { event_rx },
        }
    }

    /// Separates the cloneable command handle from the single event stream.
    pub fn split(self) -> (ConnectionHandle, ConnectionEventReceiver) {
        (self.handle, self.events)
    }
}

enum PendingResult<T> {
    Completed(Result<T, SshError>),
    Disconnect,
}

struct ConnectionWorkerContext {
    commands: mpsc::UnboundedReceiver<ConnectionCommand>,
    host_key_decisions: mpsc::UnboundedReceiver<HostKeyDecision>,
    host_key_verification_pending: Arc<AtomicBool>,
    events: mpsc::Sender<ConnectionEvent>,
    transfer_rate_limiter: Arc<TransferRateLimiter>,
}

async fn wait_for_operation<T, F>(
    operation: F,
    commands: &mut mpsc::UnboundedReceiver<ConnectionCommand>,
    latest_size: &mut PtySize,
) -> PendingResult<T>
where
    F: Future<Output = Result<T, SshError>>,
{
    tokio::pin!(operation);

    loop {
        tokio::select! {
            result = &mut operation => {
                return PendingResult::Completed(result);
            }

            command = commands.recv() => {
                match command {
                    Some(ConnectionCommand::Resize(size)) => {
                        *latest_size = size;
                    }
                    Some(ConnectionCommand::Input(_)) => {
                        // Keyboard input is ignored until the shell is ready.
                    }
                    Some(
                        ConnectionCommand::ReadDirectory { .. }
                        | ConnectionCommand::ReadDirectoryTree { .. }
                        | ConnectionCommand::ReadFile { .. }
                        | ConnectionCommand::WriteFile { .. }
                        | ConnectionCommand::CreateFile { .. }
                        | ConnectionCommand::CreateDirectories { .. }
                        | ConnectionCommand::DeletePaths { .. }
                        | ConnectionCommand::UploadFile { .. }
                        | ConnectionCommand::DownloadFile { .. }
                        | ConnectionCommand::CancelTransfer { .. }
                        | ConnectionCommand::SetPerformanceMonitoring(_),
                    ) => {
                        // Subsystem requests are ignored until authentication completes.
                    }
                    Some(ConnectionCommand::Disconnect) | None => {
                        return PendingResult::Disconnect;
                    }
                }
            }
        }
    }
}

async fn wait_for_host_key_decision(
    decisions: &mut mpsc::UnboundedReceiver<HostKeyDecision>,
    commands: &mut mpsc::UnboundedReceiver<ConnectionCommand>,
    latest_size: &mut PtySize,
) -> PendingResult<HostKeyDecision> {
    loop {
        tokio::select! {
            decision = decisions.recv() => {
                return PendingResult::Completed(decision.ok_or_else(|| {
                    SshError::new(
                        SshErrorKind::InvalidState,
                        "SSH host-key verification channel closed",
                    )
                }));
            }

            command = commands.recv() => {
                match command {
                    Some(ConnectionCommand::Resize(size)) => {
                        *latest_size = size;
                    }
                    Some(ConnectionCommand::Input(_)) => {
                        // Keyboard input is ignored until the shell is ready.
                    }
                    Some(
                        ConnectionCommand::ReadDirectory { .. }
                        | ConnectionCommand::ReadDirectoryTree { .. }
                        | ConnectionCommand::ReadFile { .. }
                        | ConnectionCommand::WriteFile { .. }
                        | ConnectionCommand::CreateFile { .. }
                        | ConnectionCommand::CreateDirectories { .. }
                        | ConnectionCommand::DeletePaths { .. }
                        | ConnectionCommand::UploadFile { .. }
                        | ConnectionCommand::DownloadFile { .. }
                        | ConnectionCommand::CancelTransfer { .. }
                        | ConnectionCommand::SetPerformanceMonitoring(_),
                    ) => {
                        // Subsystem requests are ignored until authentication completes.
                    }
                    Some(ConnectionCommand::Disconnect) | None => {
                        return PendingResult::Disconnect;
                    }
                }
            }
        }
    }
}

fn coalesce_queued_resizes(
    initial_size: PtySize,
    commands: &mut mpsc::UnboundedReceiver<ConnectionCommand>,
) -> (PtySize, Option<ConnectionCommand>) {
    let mut latest_size = initial_size;

    while let Ok(command) = commands.try_recv() {
        match command {
            ConnectionCommand::Resize(size) => latest_size = size,
            command => return (latest_size, Some(command)),
        }
    }

    (latest_size, None)
}

async fn run_connection_plan(
    plan: ConnectionPlan,
    mut latest_size: PtySize,
    context: ConnectionWorkerContext,
) {
    let ConnectionWorkerContext {
        mut commands,
        mut host_key_decisions,
        host_key_verification_pending,
        events,
        transfer_rate_limiter,
    } = context;
    let target_profile = plan.target_profile().clone();
    let mut session = SshSession::new(target_profile.clone());

    if let Err(error) = session.begin_connect() {
        report_failure(&mut session, error, &events).await;
        return;
    }

    if !send_state(&events, SessionState::Connecting).await {
        return;
    }

    if let Err(error) = plan.validate() {
        report_failure(&mut session, error, &events).await;
        return;
    }
    let (target, jumps, proxy) = plan.into_parts();
    let jump_total = jumps.len();
    let mut steps = jumps
        .into_iter()
        .enumerate()
        .map(|(index, step)| {
            let stage = ConnectionStage::Jump {
                index: index + 1,
                total: jump_total,
                profile_id: step.profile.id.clone(),
            };
            (step, stage)
        })
        .collect::<Vec<_>>();
    steps.push((
        target,
        ConnectionStage::Target {
            profile_id: target_profile.id.clone(),
        },
    ));

    let mut established: Vec<SshTransport> = Vec::new();
    let mut authentication_started = false;
    for (step_index, (step, stage)) in steps.into_iter().enumerate() {
        let stage_started_at = Instant::now();
        let crate::ConnectionStep { profile, auth } = step;
        if step_index == 0
            && proxy.is_some()
            && events
                .send(ConnectionEvent::ConnectionStageChanged(
                    ConnectionStage::Proxy,
                ))
                .await
                .is_err()
        {
            return;
        }
        if events
            .send(ConnectionEvent::ConnectionStageChanged(stage.clone()))
            .await
            .is_err()
        {
            SshTransport::disconnect_established(&established).await;
            return;
        }

        let mut transport = loop {
            let opened = if step_index == 0 {
                wait_for_operation(
                    SshTransport::open_first(&profile, proxy.as_ref()),
                    &mut commands,
                    &mut latest_size,
                )
                .await
            } else {
                wait_for_operation(
                    established
                        .last()
                        .expect("a previous jump transport exists")
                        .open_via(&profile),
                    &mut commands,
                    &mut latest_size,
                )
                .await
            };
            match opened {
                PendingResult::Completed(Ok(TransportOpen::Connected(transport))) => {
                    break transport;
                }
                PendingResult::Completed(Ok(TransportOpen::UnknownHostKey(pending))) => {
                    host_key_verification_pending.store(true, Ordering::Release);
                    if events
                        .send(ConnectionEvent::HostKeyVerificationRequired {
                            stage: stage.clone(),
                            info: pending.info().clone(),
                        })
                        .await
                        .is_err()
                    {
                        host_key_verification_pending.store(false, Ordering::Release);
                        SshTransport::disconnect_established(&established).await;
                        return;
                    }

                    let decision = wait_for_host_key_decision(
                        &mut host_key_decisions,
                        &mut commands,
                        &mut latest_size,
                    )
                    .await;
                    host_key_verification_pending.store(false, Ordering::Release);

                    match decision {
                        PendingResult::Completed(Ok(HostKeyDecision::Trust)) => {
                            match wait_for_operation(
                                pending.trust(),
                                &mut commands,
                                &mut latest_size,
                            )
                            .await
                            {
                                PendingResult::Completed(Ok(())) => continue,
                                PendingResult::Completed(Err(error)) => {
                                    SshTransport::disconnect_established(&established).await;
                                    report_failure(
                                        &mut session,
                                        error.at_stage(stage.clone()),
                                        &events,
                                    )
                                    .await;
                                    return;
                                }
                                PendingResult::Disconnect => {
                                    SshTransport::disconnect_established(&established).await;
                                    finish_disconnection(&mut session, None, None, &events).await;
                                    return;
                                }
                            }
                        }
                        PendingResult::Completed(Ok(HostKeyDecision::Reject)) => {
                            SshTransport::disconnect_established(&established).await;
                            report_failure(
                                &mut session,
                                pending.rejected_error().at_stage(stage.clone()),
                                &events,
                            )
                            .await;
                            return;
                        }
                        PendingResult::Completed(Err(error)) => {
                            SshTransport::disconnect_established(&established).await;
                            report_failure(&mut session, error.at_stage(stage.clone()), &events)
                                .await;
                            return;
                        }
                        PendingResult::Disconnect => {
                            SshTransport::disconnect_established(&established).await;
                            finish_disconnection(&mut session, None, None, &events).await;
                            return;
                        }
                    }
                }
                PendingResult::Completed(Err(error)) => {
                    SshTransport::disconnect_established(&established).await;
                    report_failure(&mut session, error.at_stage(stage.clone()), &events).await;
                    return;
                }
                PendingResult::Disconnect => {
                    SshTransport::disconnect_established(&established).await;
                    finish_disconnection(&mut session, None, None, &events).await;
                    return;
                }
            }
        };
        tracing::info!(
            stage = connection_stage_name(&stage),
            elapsed_ms = stage_started_at.elapsed().as_millis() as u64,
            result = "success",
            "SSH transport established"
        );

        if !authentication_started {
            if let Err(error) = session.begin_authentication() {
                let _ = transport.disconnect().await;
                SshTransport::disconnect_established(&established).await;
                report_failure(&mut session, error, &events).await;
                return;
            }
            authentication_started = true;
            if !send_state(&events, SessionState::Authenticating).await {
                let _ = transport.disconnect().await;
                SshTransport::disconnect_established(&established).await;
                return;
            }
        }

        let auth_kind = auth.kind();
        let authentication_started_at = Instant::now();
        match wait_for_operation(
            transport.authenticate(profile.username.as_str(), auth),
            &mut commands,
            &mut latest_size,
        )
        .await
        {
            PendingResult::Completed(Ok(())) => {
                tracing::info!(
                    stage = connection_stage_name(&stage),
                    authentication = ?auth_kind,
                    elapsed_ms = authentication_started_at.elapsed().as_millis() as u64,
                    result = "success",
                    "SSH authentication completed"
                );
                if events
                    .send(ConnectionEvent::AuthenticationSucceeded {
                        stage: stage.clone(),
                        method: auth_kind,
                    })
                    .await
                    .is_err()
                {
                    let _ = transport.disconnect().await;
                    SshTransport::disconnect_established(&established).await;
                    return;
                }
            }
            PendingResult::Completed(Err(error)) => {
                tracing::warn!(
                    stage = connection_stage_name(&stage),
                    authentication = ?auth_kind,
                    elapsed_ms = authentication_started_at.elapsed().as_millis() as u64,
                    result = "failed",
                    "SSH authentication failed"
                );
                let _ = transport.disconnect().await;
                SshTransport::disconnect_established(&established).await;
                report_failure(&mut session, error.at_stage(stage), &events).await;
                return;
            }
            PendingResult::Disconnect => {
                let _ = transport.disconnect().await;
                SshTransport::disconnect_established(&established).await;
                finish_disconnection(&mut session, None, None, &events).await;
                return;
            }
        }
        established.push(transport);
    }

    let transport = SshTransport::combine_chain(established);

    let requested_size = latest_size;
    let shell = match wait_for_operation(
        transport.open_shell(requested_size),
        &mut commands,
        &mut latest_size,
    )
    .await
    {
        PendingResult::Completed(Ok(shell)) => shell,
        PendingResult::Completed(Err(error)) => {
            report_failure(&mut session, error, &events).await;
            let _ = transport.disconnect().await;
            return;
        }
        PendingResult::Disconnect => {
            finish_disconnection(&mut session, Some(&transport), None, &events).await;
            return;
        }
    };

    let (mut reader, writer) = shell.split();

    if latest_size != requested_size
        && let Err(error) = writer.resize(latest_size).await
    {
        report_failure(&mut session, error, &events).await;
        close_resources(&transport, Some(&writer)).await;
        return;
    }

    if events
        .send(ConnectionEvent::Resized(latest_size))
        .await
        .is_err()
    {
        close_resources(&transport, Some(&writer)).await;
        return;
    }

    if let Err(error) = session.mark_connected() {
        report_failure(&mut session, error, &events).await;
        close_resources(&transport, Some(&writer)).await;
        return;
    }

    if !send_state(&events, SessionState::Connected).await {
        close_resources(&transport, Some(&writer)).await;
        return;
    }

    let transport = Arc::new(transport);
    let mut pending_command = None;
    let mut sftp_worker = None;
    let mut sftp_available = None;
    let mut sftp_probe_pending = true;
    let mut sftp_probe = Box::pin(transport.check_sftp_availability());
    let mut shell_probe_pending = true;
    let mut shell_probe = Box::pin(transport.detect_shell());
    let mut detected_shell = None;
    let mut shell_output_seen = false;
    let mut shell_integration_deadline = None;
    let mut performance_monitor = None;

    loop {
        let command = if let Some(command) = pending_command.take() {
            Some(command)
        } else {
            tokio::select! {
                command = commands.recv() => command,
                availability = &mut sftp_probe, if sftp_probe_pending => {
                    sftp_probe_pending = false;
                    let (available, message) = match availability {
                        Ok(true) => (true, None),
                        Ok(false) => (
                            false,
                            Some("SFTP is not installed or configured on this server".into()),
                        ),
                        Err(error) => (
                            false,
                            Some(format!("Could not verify SFTP availability: {error}")),
                        ),
                    };
                    sftp_available = Some(available);
                    if events
                        .send(ConnectionEvent::SftpAvailabilityChanged {
                            available,
                            message,
                        })
                        .await
                        .is_err()
                    {
                        close_resources(&transport, Some(&writer)).await;
                        return;
                    }
                    continue;
                }
                shell = &mut shell_probe, if shell_probe_pending => {
                    shell_probe_pending = false;
                    if let Ok(Some(shell)) = shell {
                        detected_shell = Some(shell);
                        if shell_output_seen {
                            shell_integration_deadline =
                                Some(Instant::now() + SHELL_INTEGRATION_QUIET_PERIOD);
                        }
                    }
                    continue;
                }
                _ = async {
                    if let Some(deadline) = shell_integration_deadline {
                        sleep_until(deadline).await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                }, if shell_integration_deadline.is_some() => {
                    shell_integration_deadline = None;
                    if let Some(shell) = detected_shell.take() {
                        reader.begin_integration_filter();
                        if let Err(error) = writer.install_cwd_hook(shell).await {
                            report_failure(&mut session, error, &events).await;
                            close_resources(&transport, Some(&writer)).await;
                            return;
                        }
                    }
                    continue;
                }
                shell_event = reader.next_event() => {
                    let is_closed = matches!(&shell_event, ShellEvent::Closed);
                    let is_output = matches!(
                        &shell_event,
                        ShellEvent::Output(_) | ShellEvent::ExtendedOutput { .. }
                    );
                    if is_output {
                        shell_output_seen = true;
                        if detected_shell.is_some() {
                            shell_integration_deadline =
                                Some(Instant::now() + SHELL_INTEGRATION_QUIET_PERIOD);
                        }
                    }

                    if events
                        .send(ConnectionEvent::Shell(shell_event))
                        .await
                        .is_err()
                    {
                        close_resources(&transport, Some(&writer)).await;
                        return;
                    }

                    if is_closed {
                        finish_disconnection(
                            &mut session,
                            Some(&transport),
                            Some(&writer),
                            &events,
                        )
                        .await;
                        return;
                    }

                    continue;
                }
            }
        };

        match command {
            Some(ConnectionCommand::Input(data)) => {
                if let Err(error) = writer.send_input(data).await {
                    report_failure(&mut session, error, &events).await;
                    close_resources(&transport, Some(&writer)).await;
                    return;
                }
            }
            Some(ConnectionCommand::Resize(size)) => {
                let (latest_size, next_command) = coalesce_queued_resizes(size, &mut commands);
                pending_command = next_command;

                if let Err(error) = writer.resize(latest_size).await {
                    report_failure(&mut session, error, &events).await;
                    close_resources(&transport, Some(&writer)).await;
                    return;
                }

                if events
                    .send(ConnectionEvent::Resized(latest_size))
                    .await
                    .is_err()
                {
                    close_resources(&transport, Some(&writer)).await;
                    return;
                }
            }
            Some(ConnectionCommand::ReadDirectory { request_id, path }) => {
                if sftp_available != Some(true) {
                    let error = SshError::new(
                        SshErrorKind::Sftp,
                        if sftp_probe_pending {
                            "SFTP availability is still being checked"
                        } else {
                            "SFTP is unavailable on this server"
                        },
                    );
                    if events
                        .send(ConnectionEvent::SftpFailed {
                            request_id,
                            path,
                            operation: SftpOperation::ReadDirectory,
                            error,
                        })
                        .await
                        .is_err()
                    {
                        close_resources(&transport, Some(&writer)).await;
                        return;
                    }
                    continue;
                }
                if sftp_worker.is_none() {
                    match transport.open_sftp().await {
                        Ok(session) => {
                            sftp_worker = Some(SftpWorkerHandle::spawn_with_limiter(
                                session,
                                events.clone(),
                                transfer_rate_limiter.clone(),
                            ));
                        }
                        Err(error) => {
                            if events
                                .send(ConnectionEvent::SftpFailed {
                                    request_id,
                                    path,
                                    operation: SftpOperation::ReadDirectory,
                                    error,
                                })
                                .await
                                .is_err()
                            {
                                close_resources(&transport, Some(&writer)).await;
                                return;
                            }
                            continue;
                        }
                    }
                }

                if let Some(worker) = sftp_worker.as_ref()
                    && let Err(error) = worker.read_directory(request_id, path.clone())
                    && events
                        .send(ConnectionEvent::SftpFailed {
                            request_id,
                            path,
                            operation: SftpOperation::ReadDirectory,
                            error,
                        })
                        .await
                        .is_err()
                {
                    close_resources(&transport, Some(&writer)).await;
                    return;
                }
            }
            Some(ConnectionCommand::ReadDirectoryTree { request_id, path }) => {
                if sftp_worker.is_none() {
                    match transport.open_sftp().await {
                        Ok(session) => {
                            sftp_worker = Some(SftpWorkerHandle::spawn_with_limiter(
                                session,
                                events.clone(),
                                transfer_rate_limiter.clone(),
                            ));
                        }
                        Err(error) => {
                            if events
                                .send(ConnectionEvent::SftpFailed {
                                    request_id,
                                    path,
                                    operation: SftpOperation::ReadDirectoryTree,
                                    error,
                                })
                                .await
                                .is_err()
                            {
                                close_resources(&transport, Some(&writer)).await;
                                return;
                            }
                            continue;
                        }
                    }
                }

                if let Some(worker) = sftp_worker.as_ref()
                    && let Err(error) = worker.read_directory_tree(request_id, path.clone())
                    && events
                        .send(ConnectionEvent::SftpFailed {
                            request_id,
                            path,
                            operation: SftpOperation::ReadDirectoryTree,
                            error,
                        })
                        .await
                        .is_err()
                {
                    close_resources(&transport, Some(&writer)).await;
                    return;
                }
            }
            Some(ConnectionCommand::ReadFile { request_id, path }) => {
                if sftp_worker.is_none() {
                    match transport.open_sftp().await {
                        Ok(session) => {
                            sftp_worker = Some(SftpWorkerHandle::spawn_with_limiter(
                                session,
                                events.clone(),
                                transfer_rate_limiter.clone(),
                            ));
                        }
                        Err(error) => {
                            if events
                                .send(ConnectionEvent::SftpFailed {
                                    request_id,
                                    path,
                                    operation: SftpOperation::ReadFile,
                                    error,
                                })
                                .await
                                .is_err()
                            {
                                close_resources(&transport, Some(&writer)).await;
                                return;
                            }
                            continue;
                        }
                    }
                }

                if let Some(worker) = sftp_worker.as_ref()
                    && let Err(error) = worker.read_file(request_id, path.clone())
                    && events
                        .send(ConnectionEvent::SftpFailed {
                            request_id,
                            path,
                            operation: SftpOperation::ReadFile,
                            error,
                        })
                        .await
                        .is_err()
                {
                    close_resources(&transport, Some(&writer)).await;
                    return;
                }
            }
            Some(ConnectionCommand::WriteFile {
                request_id,
                path,
                expected_contents,
                contents,
            }) => {
                if sftp_worker.is_none() {
                    match transport.open_sftp().await {
                        Ok(session) => {
                            sftp_worker = Some(SftpWorkerHandle::spawn_with_limiter(
                                session,
                                events.clone(),
                                transfer_rate_limiter.clone(),
                            ));
                        }
                        Err(error) => {
                            if events
                                .send(ConnectionEvent::SftpFailed {
                                    request_id,
                                    path,
                                    operation: SftpOperation::WriteFile,
                                    error,
                                })
                                .await
                                .is_err()
                            {
                                close_resources(&transport, Some(&writer)).await;
                                return;
                            }
                            continue;
                        }
                    }
                }

                if let Some(worker) = sftp_worker.as_ref()
                    && let Err(error) =
                        worker.write_file(request_id, path.clone(), expected_contents, contents)
                    && events
                        .send(ConnectionEvent::SftpFailed {
                            request_id,
                            path,
                            operation: SftpOperation::WriteFile,
                            error,
                        })
                        .await
                        .is_err()
                {
                    close_resources(&transport, Some(&writer)).await;
                    return;
                }
            }
            Some(ConnectionCommand::CreateFile { request_id, path }) => {
                if sftp_worker.is_none() {
                    match transport.open_sftp().await {
                        Ok(session) => {
                            sftp_worker = Some(SftpWorkerHandle::spawn_with_limiter(
                                session,
                                events.clone(),
                                transfer_rate_limiter.clone(),
                            ));
                        }
                        Err(error) => {
                            if events
                                .send(ConnectionEvent::SftpFailed {
                                    request_id,
                                    path,
                                    operation: SftpOperation::CreateFile,
                                    error,
                                })
                                .await
                                .is_err()
                            {
                                close_resources(&transport, Some(&writer)).await;
                                return;
                            }
                            continue;
                        }
                    }
                }

                if let Some(worker) = sftp_worker.as_ref()
                    && let Err(error) = worker.create_file(request_id, path.clone())
                    && events
                        .send(ConnectionEvent::SftpFailed {
                            request_id,
                            path,
                            operation: SftpOperation::CreateFile,
                            error,
                        })
                        .await
                        .is_err()
                {
                    close_resources(&transport, Some(&writer)).await;
                    return;
                }
            }
            Some(ConnectionCommand::CreateDirectories { request_id, paths }) => {
                let error_path = paths.first().cloned().unwrap_or_default();
                if sftp_worker.is_none() {
                    match transport.open_sftp().await {
                        Ok(session) => {
                            sftp_worker = Some(SftpWorkerHandle::spawn_with_limiter(
                                session,
                                events.clone(),
                                transfer_rate_limiter.clone(),
                            ));
                        }
                        Err(error) => {
                            if events
                                .send(ConnectionEvent::SftpFailed {
                                    request_id,
                                    path: error_path,
                                    operation: SftpOperation::CreateDirectory,
                                    error,
                                })
                                .await
                                .is_err()
                            {
                                close_resources(&transport, Some(&writer)).await;
                                return;
                            }
                            continue;
                        }
                    }
                }

                if let Some(worker) = sftp_worker.as_ref()
                    && let Err(error) = worker.create_directories(request_id, paths)
                    && events
                        .send(ConnectionEvent::SftpFailed {
                            request_id,
                            path: error_path,
                            operation: SftpOperation::CreateDirectory,
                            error,
                        })
                        .await
                        .is_err()
                {
                    close_resources(&transport, Some(&writer)).await;
                    return;
                }
            }
            Some(ConnectionCommand::DeletePaths { request_id, paths }) => {
                let error_path = paths.first().cloned().unwrap_or_default();
                if sftp_worker.is_none() {
                    match transport.open_sftp().await {
                        Ok(session) => {
                            sftp_worker = Some(SftpWorkerHandle::spawn_with_limiter(
                                session,
                                events.clone(),
                                transfer_rate_limiter.clone(),
                            ));
                        }
                        Err(error) => {
                            if events
                                .send(ConnectionEvent::SftpFailed {
                                    request_id,
                                    path: error_path,
                                    operation: SftpOperation::DeletePaths,
                                    error,
                                })
                                .await
                                .is_err()
                            {
                                close_resources(&transport, Some(&writer)).await;
                                return;
                            }
                            continue;
                        }
                    }
                }

                if let Some(worker) = sftp_worker.as_ref()
                    && let Err(error) = worker.delete_paths(request_id, paths)
                    && events
                        .send(ConnectionEvent::SftpFailed {
                            request_id,
                            path: error_path,
                            operation: SftpOperation::DeletePaths,
                            error,
                        })
                        .await
                        .is_err()
                {
                    close_resources(&transport, Some(&writer)).await;
                    return;
                }
            }
            Some(ConnectionCommand::UploadFile {
                transfer_id,
                local_path,
                remote_path,
                overwrite,
            }) => {
                if sftp_worker.is_none() {
                    match transport.open_sftp().await {
                        Ok(sftp_session) => {
                            sftp_worker = Some(SftpWorkerHandle::spawn_with_limiter(
                                sftp_session,
                                events.clone(),
                                transfer_rate_limiter.clone(),
                            ));
                        }
                        Err(error) => {
                            if events
                                .send(ConnectionEvent::SftpFailed {
                                    request_id: transfer_id,
                                    path: remote_path,
                                    operation: SftpOperation::UploadFile,
                                    error,
                                })
                                .await
                                .is_err()
                            {
                                close_resources(&transport, Some(&writer)).await;
                                return;
                            }
                            continue;
                        }
                    }
                }

                if let Some(worker) = sftp_worker.as_ref()
                    && let Err(error) =
                        worker.upload_file(transfer_id, local_path, remote_path.clone(), overwrite)
                    && events
                        .send(ConnectionEvent::SftpFailed {
                            request_id: transfer_id,
                            path: remote_path,
                            operation: SftpOperation::UploadFile,
                            error,
                        })
                        .await
                        .is_err()
                {
                    close_resources(&transport, Some(&writer)).await;
                    return;
                }
            }
            Some(ConnectionCommand::DownloadFile {
                transfer_id,
                remote_path,
                local_path,
                overwrite,
            }) => {
                if sftp_worker.is_none() {
                    match transport.open_sftp().await {
                        Ok(sftp_session) => {
                            sftp_worker = Some(SftpWorkerHandle::spawn_with_limiter(
                                sftp_session,
                                events.clone(),
                                transfer_rate_limiter.clone(),
                            ));
                        }
                        Err(error) => {
                            if events
                                .send(ConnectionEvent::SftpFailed {
                                    request_id: transfer_id,
                                    path: remote_path,
                                    operation: SftpOperation::DownloadFile,
                                    error,
                                })
                                .await
                                .is_err()
                            {
                                close_resources(&transport, Some(&writer)).await;
                                return;
                            }
                            continue;
                        }
                    }
                }

                if let Some(worker) = sftp_worker.as_ref()
                    && let Err(error) = worker.download_file(
                        transfer_id,
                        remote_path.clone(),
                        local_path,
                        overwrite,
                    )
                    && events
                        .send(ConnectionEvent::SftpFailed {
                            request_id: transfer_id,
                            path: remote_path,
                            operation: SftpOperation::DownloadFile,
                            error,
                        })
                        .await
                        .is_err()
                {
                    close_resources(&transport, Some(&writer)).await;
                    return;
                }
            }
            Some(ConnectionCommand::CancelTransfer { transfer_id }) => {
                if let Some(worker) = sftp_worker.as_ref()
                    && let Err(error) = worker.cancel_transfer(transfer_id)
                    && events
                        .send(ConnectionEvent::SftpFailed {
                            request_id: transfer_id,
                            path: String::new(),
                            operation: SftpOperation::CancelTransfer,
                            error,
                        })
                        .await
                        .is_err()
                {
                    close_resources(&transport, Some(&writer)).await;
                    return;
                }
            }
            Some(ConnectionCommand::SetPerformanceMonitoring(enabled)) => {
                if enabled && performance_monitor.is_none() {
                    performance_monitor = Some(PerformanceMonitorHandle::spawn(
                        transport.clone(),
                        events.clone(),
                    ));
                } else if !enabled {
                    performance_monitor = None;
                }
            }
            Some(ConnectionCommand::Disconnect) | None => {
                finish_disconnection(&mut session, Some(&transport), Some(&writer), &events).await;
                return;
            }
        }
    }
}

async fn send_state(events: &mpsc::Sender<ConnectionEvent>, state: SessionState) -> bool {
    events
        .send(ConnectionEvent::StateChanged(state))
        .await
        .is_ok()
}

async fn report_failure(
    session: &mut SshSession,
    error: SshError,
    events: &mpsc::Sender<ConnectionEvent>,
) {
    tracing::warn!(
        stage = error.stage().map(connection_stage_name).unwrap_or("unknown"),
        error_kind = ?error.kind(),
        "SSH connection failed"
    );
    session.mark_failed(error.clone());
    let _ = events.send(ConnectionEvent::Failed(error)).await;
}

fn connection_stage_name(stage: &ConnectionStage) -> &'static str {
    match stage {
        ConnectionStage::Proxy => "proxy",
        ConnectionStage::Jump { .. } => "jump",
        ConnectionStage::Target { .. } => "target",
    }
}

async fn finish_disconnection(
    session: &mut SshSession,
    transport: Option<&SshTransport>,
    writer: Option<&SshShellWriter>,
    events: &mpsc::Sender<ConnectionEvent>,
) {
    if session.begin_disconnect().is_ok() {
        let _ = send_state(events, SessionState::Disconnecting).await;
    }

    if let Some(transport) = transport {
        close_resources(transport, writer).await;
    }

    if session.mark_disconnected().is_ok() {
        let _ = send_state(events, SessionState::Disconnected).await;
    }
}

async fn close_resources(transport: &SshTransport, writer: Option<&SshShellWriter>) {
    if let Some(writer) = writer {
        let _ = writer.close().await;
    }

    let _ = transport.disconnect().await;
}

#[cfg(test)]
mod tests;
