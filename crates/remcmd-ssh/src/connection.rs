use std::future::Future;

use remcmd_core::ConnectionProfile;
use tokio::{runtime::Handle, sync::mpsc};

use crate::{
    AuthMethod, PtySize, SessionState, ShellEvent, SshError, SshErrorKind, SshSession,
    SshShellWriter, SshTransport,
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

    /// Requests an orderly shell and transport shutdown.
    Disconnect,
}

/// Events sent from one SSH worker back to the application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionEvent {
    /// Reports a successful lifecycle transition.
    StateChanged(SessionState),

    /// Carries output or lifecycle information from the remote shell.
    Shell(ShellEvent),

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

    /// Requests an orderly disconnection.
    pub fn disconnect(&self) -> Result<(), SshError> {
        self.send(ConnectionCommand::Disconnect)
    }

    fn send(&self, command: ConnectionCommand) -> Result<(), SshError> {
        self.command_tx.send(command).map_err(|_| {
            SshError::new(
                SshErrorKind::InvalidState,
                "SSH connection task is not running",
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
        let (event_tx, event_rx) = mpsc::channel(EVENT_CHANNEL_CAPACITY);

        runtime.spawn(run_connection(
            profile,
            auth,
            initial_size,
            command_rx,
            event_tx,
        ));

        Self {
            handle: ConnectionHandle { command_tx },
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
                    Some(ConnectionCommand::Disconnect) | None => {
                        return PendingResult::Disconnect;
                    }
                }
            }
        }
    }
}

async fn run_connection(
    profile: ConnectionProfile,
    auth: AuthMethod,
    mut latest_size: PtySize,
    mut commands: mpsc::UnboundedReceiver<ConnectionCommand>,
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

    let mut transport = match wait_for_operation(
        SshTransport::open(&profile),
        &mut commands,
        &mut latest_size,
    )
    .await
    {
        PendingResult::Completed(Ok(transport)) => transport,
        PendingResult::Completed(Err(error)) => {
            report_failure(&mut session, error, &events).await;
            return;
        }
        PendingResult::Disconnect => {
            finish_disconnection(&mut session, None, None, &events).await;
            return;
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

    if let Err(error) = session.mark_connected() {
        report_failure(&mut session, error, &events).await;
        close_resources(&transport, Some(&writer)).await;
        return;
    }

    if !send_state(&events, SessionState::Connected).await {
        close_resources(&transport, Some(&writer)).await;
        return;
    }

    loop {
        tokio::select! {
            command = commands.recv() => {
                match command {
                    Some(ConnectionCommand::Input(data)) => {
                        if let Err(error) = writer.send_input(data).await {
                            report_failure(&mut session, error, &events).await;
                            close_resources(&transport, Some(&writer)).await;
                            return;
                        }
                    }
                    Some(ConnectionCommand::Resize(size)) => {
                        if let Err(error) = writer.resize(size).await {
                            report_failure(&mut session, error, &events).await;
                            close_resources(&transport, Some(&writer)).await;
                            return;
                        }
                    }
                    Some(ConnectionCommand::Disconnect) | None => {
                        finish_disconnection(
                            &mut session,
                            Some(&transport),
                            Some(&writer),
                            &events,
                        )
                        .await;
                        return;
                    }
                }
            }

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
