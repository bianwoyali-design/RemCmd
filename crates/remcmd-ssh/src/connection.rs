use std::{
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use remcmd_core::ConnectionProfile;
use tokio::{runtime::Handle, sync::mpsc};

use crate::{
    AuthMethod, HostKeyInfo, PtySize, RemoteDirectory, RemoteFile, SessionState, SftpOperation,
    ShellEvent, SshError, SshErrorKind, SshSession, SshShellWriter, SshTransport,
    host_key::HostKeyDecision, sftp::SftpWorkerHandle, transport::TransportOpen,
};

const EVENT_CHANNEL_CAPACITY: usize = 256;

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

    /// Reads one remote file through an SFTP subsystem channel.
    ReadFile { request_id: u64, path: String },

    /// Replaces a remote file if its contents have not changed since it was read.
    WriteFile {
        request_id: u64,
        path: String,
        expected_contents: Vec<u8>,
        contents: Vec<u8>,
    },

    /// Requests an orderly shell and transport shutdown.
    Disconnect,
}

/// Events sent from one SSH worker back to the application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionEvent {
    /// Reports a successful lifecycle transition.
    StateChanged(SessionState),

    /// Pauses the SSH handshake until the user verifies an unknown server key.
    HostKeyVerificationRequired(HostKeyInfo),

    /// Confirms that the remote PTY accepted a terminal resize.
    Resized(PtySize),

    /// Carries output or lifecycle information from the remote shell.
    Shell(ShellEvent),

    /// Returns one canonical remote path and its directory entries.
    DirectoryRead {
        request_id: u64,
        directory: RemoteDirectory,
    },

    /// Returns one canonical remote path and its file contents.
    FileRead { request_id: u64, file: RemoteFile },

    /// Confirms that a remote file was replaced and returns its saved contents.
    FileWritten { request_id: u64, file: RemoteFile },

    /// Reports an SFTP operation failure without failing the SSH shell.
    SftpFailed {
        request_id: u64,
        path: String,
        operation: SftpOperation,
        error: SshError,
    },

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
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let (host_key_decision_tx, host_key_decision_rx) = mpsc::unbounded_channel();
        let host_key_verification_pending = Arc::new(AtomicBool::new(false));
        let (event_tx, event_rx) = mpsc::channel(EVENT_CHANNEL_CAPACITY);

        runtime.spawn(run_connection(
            profile,
            auth,
            initial_size,
            command_rx,
            host_key_decision_rx,
            host_key_verification_pending.clone(),
            event_tx,
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
                        | ConnectionCommand::ReadFile { .. }
                        | ConnectionCommand::WriteFile { .. },
                    ) => {
                        // SFTP requests are ignored until authentication completes.
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
                        | ConnectionCommand::ReadFile { .. }
                        | ConnectionCommand::WriteFile { .. },
                    ) => {
                        // SFTP requests are ignored until authentication completes.
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

async fn run_connection(
    profile: ConnectionProfile,
    auth: AuthMethod,
    mut latest_size: PtySize,
    mut commands: mpsc::UnboundedReceiver<ConnectionCommand>,
    mut host_key_decisions: mpsc::UnboundedReceiver<HostKeyDecision>,
    host_key_verification_pending: Arc<AtomicBool>,
    events: mpsc::Sender<ConnectionEvent>,
) {
    let mut session = SshSession::new(profile.clone());

    if let Err(error) = session.begin_connect() {
        report_failure(&mut session, error, &events).await;
        return;
    }

    if !send_state(&events, SessionState::Connecting).await {
        return;
    }

    let mut transport = loop {
        match wait_for_operation(
            SshTransport::open(&profile),
            &mut commands,
            &mut latest_size,
        )
        .await
        {
            PendingResult::Completed(Ok(TransportOpen::Connected(transport))) => break transport,
            PendingResult::Completed(Ok(TransportOpen::UnknownHostKey(pending))) => {
                host_key_verification_pending.store(true, Ordering::Release);
                if events
                    .send(ConnectionEvent::HostKeyVerificationRequired(
                        pending.info().clone(),
                    ))
                    .await
                    .is_err()
                {
                    host_key_verification_pending.store(false, Ordering::Release);
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
                        match wait_for_operation(pending.trust(), &mut commands, &mut latest_size)
                            .await
                        {
                            PendingResult::Completed(Ok(())) => continue,
                            PendingResult::Completed(Err(error)) => {
                                report_failure(&mut session, error, &events).await;
                                return;
                            }
                            PendingResult::Disconnect => {
                                finish_disconnection(&mut session, None, None, &events).await;
                                return;
                            }
                        }
                    }
                    PendingResult::Completed(Ok(HostKeyDecision::Reject)) => {
                        report_failure(&mut session, pending.rejected_error(), &events).await;
                        return;
                    }
                    PendingResult::Completed(Err(error)) => {
                        report_failure(&mut session, error, &events).await;
                        return;
                    }
                    PendingResult::Disconnect => {
                        finish_disconnection(&mut session, None, None, &events).await;
                        return;
                    }
                }
            }
            PendingResult::Completed(Err(error)) => {
                report_failure(&mut session, error, &events).await;
                return;
            }
            PendingResult::Disconnect => {
                finish_disconnection(&mut session, None, None, &events).await;
                return;
            }
        }
    };

    if let Err(error) = session.begin_authentication() {
        report_failure(&mut session, error, &events).await;
        let _ = transport.disconnect().await;
        return;
    }

    if !send_state(&events, SessionState::Authenticating).await {
        let _ = transport.disconnect().await;
        return;
    }

    match wait_for_operation(
        transport.authenticate(profile.username.as_str(), auth),
        &mut commands,
        &mut latest_size,
    )
    .await
    {
        PendingResult::Completed(Ok(())) => {}
        PendingResult::Completed(Err(error)) => {
            report_failure(&mut session, error, &events).await;
            let _ = transport.disconnect().await;
            return;
        }
        PendingResult::Disconnect => {
            finish_disconnection(&mut session, Some(&transport), None, &events).await;
            return;
        }
    }

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

    let mut pending_command = None;
    let mut sftp_worker = None;

    loop {
        let command = if let Some(command) = pending_command.take() {
            Some(command)
        } else {
            tokio::select! {
                command = commands.recv() => command,
                shell_event = reader.next_event() => {
                    let is_closed = matches!(&shell_event, ShellEvent::Closed);

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
                if sftp_worker.is_none() {
                    match transport.open_sftp().await {
                        Ok(session) => {
                            sftp_worker = Some(SftpWorkerHandle::spawn(session, events.clone()));
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
            Some(ConnectionCommand::ReadFile { request_id, path }) => {
                if sftp_worker.is_none() {
                    match transport.open_sftp().await {
                        Ok(session) => {
                            sftp_worker = Some(SftpWorkerHandle::spawn(session, events.clone()));
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
                            sftp_worker = Some(SftpWorkerHandle::spawn(session, events.clone()));
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
    session.mark_failed(error.clone());
    let _ = events.send(ConnectionEvent::Failed(error)).await;
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
