use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use russh::{Channel, ChannelMsg, client};
use tokio::{
    fs,
    io::AsyncReadExt,
    sync::mpsc,
    task::JoinSet,
    time::{Duration, timeout},
};

use crate::{
    ConnectionEvent, SftpOperation, SftpTransferDirection, SshError, SshErrorKind, SshTransport,
    TransferRateLimiter,
    sftp::{
        TRANSFER_CHUNK_BYTES, TransferContext, TransferResult, remote_transfer_temporary_path,
        transfer_io_error, transfer_result_event, transfer_temporary_suffix,
    },
};

const SCP_ACK_TIMEOUT: Duration = Duration::from_secs(10);
const SCP_EXIT_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_SCP_ERROR_BYTES: usize = 16 * 1024;

enum ScpCommand {
    CreateDirectories {
        request_id: u64,
        paths: Vec<String>,
    },
    UploadFile {
        transfer_id: u64,
        local_path: PathBuf,
        remote_path: String,
        overwrite: bool,
    },
    CancelTransfer {
        transfer_id: u64,
    },
}

pub(crate) struct ScpWorkerHandle {
    command_tx: mpsc::UnboundedSender<ScpCommand>,
}

impl ScpWorkerHandle {
    pub(crate) fn spawn(
        transport: Arc<SshTransport>,
        events: mpsc::Sender<ConnectionEvent>,
        rate_limiter: Arc<TransferRateLimiter>,
    ) -> Self {
        let (command_tx, mut commands) = mpsc::unbounded_channel();

        tokio::spawn(async move {
            let cancellations = Arc::new(Mutex::new(HashMap::<u64, Arc<AtomicBool>>::new()));
            let mut transfer_tasks = JoinSet::new();

            while let Some(command) = commands.recv().await {
                while transfer_tasks.try_join_next().is_some() {}
                match command {
                    ScpCommand::CreateDirectories { request_id, paths } => {
                        let error_path = paths.first().cloned().unwrap_or_default();
                        let event = match create_directories(&transport, &paths).await {
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
                    ScpCommand::UploadFile {
                        transfer_id,
                        local_path,
                        remote_path,
                        overwrite,
                    } => {
                        let cancellation = Arc::new(AtomicBool::new(false));
                        cancellations
                            .lock()
                            .expect("SCP cancellation map should not be poisoned")
                            .insert(transfer_id, cancellation.clone());
                        let transport = transport.clone();
                        let events = events.clone();
                        let cancellations = cancellations.clone();
                        let rate_limiter = rate_limiter.clone();
                        transfer_tasks.spawn(async move {
                            let context = TransferContext::new(
                                transfer_id,
                                cancellation,
                                events.clone(),
                                rate_limiter,
                            );
                            let result = upload_file(
                                &transport,
                                &local_path,
                                &remote_path,
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
                            cancellations
                                .lock()
                                .expect("SCP cancellation map should not be poisoned")
                                .remove(&transfer_id);
                        });
                    }
                    ScpCommand::CancelTransfer { transfer_id } => {
                        if let Some(cancellation) = cancellations
                            .lock()
                            .expect("SCP cancellation map should not be poisoned")
                            .get(&transfer_id)
                        {
                            cancellation.store(true, Ordering::Release);
                        }
                    }
                }
            }

            for cancellation in cancellations
                .lock()
                .expect("SCP cancellation map should not be poisoned")
                .values()
            {
                cancellation.store(true, Ordering::Release);
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
        });

        Self { command_tx }
    }

    pub(crate) fn create_directories(
        &self,
        request_id: u64,
        paths: Vec<String>,
    ) -> Result<(), SshError> {
        self.command_tx
            .send(ScpCommand::CreateDirectories { request_id, paths })
            .map_err(|_| SshError::new(SshErrorKind::Sftp, "SCP upload worker is not running"))
    }

    pub(crate) fn upload_file(
        &self,
        transfer_id: u64,
        local_path: PathBuf,
        remote_path: String,
        overwrite: bool,
    ) -> Result<(), SshError> {
        self.command_tx
            .send(ScpCommand::UploadFile {
                transfer_id,
                local_path,
                remote_path,
                overwrite,
            })
            .map_err(|_| SshError::new(SshErrorKind::Sftp, "SCP upload worker is not running"))
    }

    pub(crate) fn cancel_transfer(&self, transfer_id: u64) -> Result<(), SshError> {
        self.command_tx
            .send(ScpCommand::CancelTransfer { transfer_id })
            .map_err(|_| SshError::new(SshErrorKind::Sftp, "SCP upload worker is not running"))
    }
}

async fn create_directories(transport: &SshTransport, paths: &[String]) -> Result<(), SshError> {
    for path in paths {
        let command = format!("mkdir -p -- {}", shell_quote(path)?);
        transport.execute(&command).await?;
    }
    Ok(())
}

async fn upload_file(
    transport: &SshTransport,
    local_path: &Path,
    remote_path: &str,
    overwrite: bool,
    context: &TransferContext,
) -> Result<TransferResult, SshError> {
    validate_remote_path(remote_path)?;
    let metadata = fs::metadata(local_path)
        .await
        .map_err(|error| transfer_io_error("reading local file metadata", error))?;
    if !metadata.is_file() {
        return Err(SshError::new(
            SshErrorKind::Sftp,
            "Only regular files can be uploaded with SCP",
        ));
    }
    let total = metadata.len();

    if !overwrite && remote_path_exists(transport, remote_path).await? {
        return Ok(TransferResult::Conflict);
    }
    if context.is_cancelled() {
        return Ok(TransferResult::Cancelled);
    }

    let temporary_path = remote_transfer_temporary_path(
        remote_path,
        &transfer_temporary_suffix(context.transfer_id()),
    );
    remove_remote_file(transport, &temporary_path).await?;

    let result = copy_to_scp_sink(transport, local_path, &temporary_path, total, context).await;
    if let Err(error) = result {
        let _ = remove_remote_file(transport, &temporary_path).await;
        if context.is_cancelled() {
            return Ok(TransferResult::Cancelled);
        }
        return Err(error);
    }
    if context.is_cancelled() {
        let _ = remove_remote_file(transport, &temporary_path).await;
        return Ok(TransferResult::Cancelled);
    }

    let install_result =
        install_remote_file(transport, &temporary_path, remote_path, overwrite).await;
    if let Err(error) = install_result {
        let _ = remove_remote_file(transport, &temporary_path).await;
        if !overwrite
            && remote_path_exists(transport, remote_path)
                .await
                .unwrap_or(false)
        {
            return Ok(TransferResult::Conflict);
        }
        return Err(error);
    }

    Ok(TransferResult::Completed(total))
}

async fn copy_to_scp_sink(
    transport: &SshTransport,
    local_path: &Path,
    remote_path: &str,
    total: u64,
    context: &TransferContext,
) -> Result<(), SshError> {
    let command = format!("scp -t -- {}", shell_quote(remote_path)?);
    let mut channel = transport.open_exec_channel(&command).await?;
    let mut protocol = ScpProtocolState::default();
    read_scp_ack(&mut channel, &mut protocol, context).await?;

    let file_name = remote_file_name(remote_path)?;
    let header = format!(
        "C{:04o} {total} {file_name}\n",
        local_file_mode(local_path).await?
    );
    channel
        .data_bytes(header.into_bytes())
        .await
        .map_err(SshError::from)?;
    read_scp_ack(&mut channel, &mut protocol, context).await?;

    let mut local_file = fs::File::open(local_path)
        .await
        .map_err(|error| transfer_io_error("opening local file", error))?;
    let mut buffer = vec![0; TRANSFER_CHUNK_BYTES];
    let mut transferred = 0_u64;
    loop {
        if context.is_cancelled() {
            let _ = channel.close().await;
            return Err(cancelled_error());
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
            let _ = channel.close().await;
            return Err(cancelled_error());
        }
        channel
            .data_bytes(buffer[..read].to_vec())
            .await
            .map_err(SshError::from)?;
        transferred = transferred.saturating_add(read as u64);
        context.report_progress(transferred, Some(total)).await?;
    }

    channel.data_bytes(vec![0]).await.map_err(SshError::from)?;
    read_scp_ack(&mut channel, &mut protocol, context).await?;
    channel.eof().await.map_err(SshError::from)?;
    wait_for_scp_exit(&mut channel, &mut protocol).await
}

#[derive(Default)]
struct ScpProtocolState {
    stdout: VecDeque<u8>,
    stderr: Vec<u8>,
    exit_status: Option<u32>,
}

async fn read_scp_ack(
    channel: &mut Channel<client::Msg>,
    state: &mut ScpProtocolState,
    context: &TransferContext,
) -> Result<(), SshError> {
    timeout(SCP_ACK_TIMEOUT, async {
        loop {
            if let Some(result) = take_scp_ack(&mut state.stdout) {
                return result;
            }
            if context.is_cancelled() {
                return Err(cancelled_error());
            }

            let message = timeout(Duration::from_millis(100), channel.wait()).await;
            match message {
                Err(_) => continue,
                Ok(Some(ChannelMsg::Success | ChannelMsg::WindowAdjusted { .. })) => {}
                Ok(Some(ChannelMsg::Failure)) => {
                    return Err(SshError::new(
                        SshErrorKind::Protocol,
                        "remote server rejected the SCP command",
                    ));
                }
                Ok(Some(ChannelMsg::Data { data })) => state.stdout.extend(data),
                Ok(Some(ChannelMsg::ExtendedData { data, .. })) => {
                    append_scp_error(&mut state.stderr, &data)
                }
                Ok(Some(ChannelMsg::ExitStatus { exit_status })) => {
                    state.exit_status = Some(exit_status);
                    if exit_status != 0 {
                        return Err(scp_command_error(state));
                    }
                }
                Ok(Some(ChannelMsg::Eof | ChannelMsg::Close)) | Ok(None) => {
                    return Err(scp_command_error(state));
                }
                Ok(Some(ChannelMsg::ExitSignal { .. })) => return Err(scp_command_error(state)),
                Ok(Some(_)) => {}
            }
        }
    })
    .await
    .map_err(|_| SshError::new(SshErrorKind::Timeout, "waiting for SCP response timed out"))?
}

fn take_scp_ack(stdout: &mut VecDeque<u8>) -> Option<Result<(), SshError>> {
    let status = *stdout.front()?;
    match status {
        0 => {
            stdout.pop_front();
            Some(Ok(()))
        }
        1 | 2 => {
            let newline = stdout.iter().position(|byte| *byte == b'\n')?;
            stdout.pop_front();
            let message = stdout.drain(..newline).collect::<Vec<_>>();
            stdout.pop_front();
            let message = String::from_utf8_lossy(&message).trim().to_owned();
            Some(Err(SshError::new(
                SshErrorKind::Sftp,
                if message.is_empty() {
                    "remote SCP process rejected the upload".to_owned()
                } else {
                    format!("remote SCP process rejected the upload: {message}")
                },
            )))
        }
        _ => Some(Err(SshError::new(
            SshErrorKind::Protocol,
            format!("remote SCP process returned invalid acknowledgement {status}"),
        ))),
    }
}

async fn wait_for_scp_exit(
    channel: &mut Channel<client::Msg>,
    state: &mut ScpProtocolState,
) -> Result<(), SshError> {
    timeout(SCP_EXIT_TIMEOUT, async {
        while let Some(message) = channel.wait().await {
            match message {
                ChannelMsg::ExitStatus { exit_status } => state.exit_status = Some(exit_status),
                ChannelMsg::ExtendedData { data, .. } => append_scp_error(&mut state.stderr, &data),
                ChannelMsg::Close => break,
                ChannelMsg::ExitSignal { .. } => return Err(scp_command_error(state)),
                _ => {}
            }
        }
        if state.exit_status.unwrap_or(0) == 0 {
            Ok(())
        } else {
            Err(scp_command_error(state))
        }
    })
    .await
    .map_err(|_| {
        SshError::new(
            SshErrorKind::Timeout,
            "waiting for SCP completion timed out",
        )
    })?
}

fn append_scp_error(output: &mut Vec<u8>, data: &[u8]) {
    let remaining = MAX_SCP_ERROR_BYTES.saturating_sub(output.len());
    output.extend_from_slice(&data[..data.len().min(remaining)]);
}

fn scp_command_error(state: &ScpProtocolState) -> SshError {
    let message = String::from_utf8_lossy(&state.stderr).trim().to_owned();
    SshError::new(
        SshErrorKind::Sftp,
        if message.is_empty() {
            "remote SCP process ended before completing the upload".to_owned()
        } else {
            format!("remote SCP process failed: {message}")
        },
    )
}

fn cancelled_error() -> SshError {
    SshError::new(SshErrorKind::Sftp, "SCP upload was cancelled")
}

async fn remote_path_exists(transport: &SshTransport, path: &str) -> Result<bool, SshError> {
    let command = format!(
        "if [ -e {} ]; then printf 'exists\\n'; else printf 'missing\\n'; fi",
        shell_quote(path)?
    );
    match transport.execute(&command).await?.as_slice() {
        b"exists\n" => Ok(true),
        b"missing\n" => Ok(false),
        _ => Err(SshError::new(
            SshErrorKind::Protocol,
            "remote server returned an invalid SCP destination response",
        )),
    }
}

async fn remove_remote_file(transport: &SshTransport, path: &str) -> Result<(), SshError> {
    transport
        .execute(&format!("rm -f -- {}", shell_quote(path)?))
        .await
        .map(|_| ())
}

async fn install_remote_file(
    transport: &SshTransport,
    temporary_path: &str,
    remote_path: &str,
    overwrite: bool,
) -> Result<(), SshError> {
    let temporary_path = shell_quote(temporary_path)?;
    let remote_path = shell_quote(remote_path)?;
    let command = if overwrite {
        format!("mv -f -- {temporary_path} {remote_path}")
    } else {
        format!(
            "if [ -e {remote_path} ]; then exit 73; else mv -- {temporary_path} {remote_path}; fi"
        )
    };
    transport.execute(&command).await.map(|_| ())
}

fn validate_remote_path(path: &str) -> Result<(), SshError> {
    if path.is_empty() || path.contains('\0') || path.contains('\r') || path.contains('\n') {
        return Err(SshError::new(
            SshErrorKind::Configuration,
            "SCP destination contains unsupported characters",
        ));
    }
    Ok(())
}

fn remote_file_name(path: &str) -> Result<&str, SshError> {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty() && !name.contains('\r') && !name.contains('\n'))
        .ok_or_else(|| {
            SshError::new(
                SshErrorKind::Configuration,
                "SCP destination does not contain a valid file name",
            )
        })
}

fn shell_quote(value: &str) -> Result<String, SshError> {
    if value.contains('\0') {
        return Err(SshError::new(
            SshErrorKind::Configuration,
            "remote path contains a null byte",
        ));
    }
    Ok(format!("'{}'", value.replace('\'', "'\\''")))
}

async fn local_file_mode(path: &Path) -> Result<u32, SshError> {
    let metadata = fs::metadata(path)
        .await
        .map_err(|error| transfer_io_error("reading local file permissions", error))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        Ok(metadata.permissions().mode() & 0o7777)
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        Ok(0o644)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rng;
    use russh::{ChannelId, server};
    use tokio::{net::TcpListener, task::JoinHandle};

    use crate::transport::ClientHandler;

    #[derive(Default)]
    struct ScpServerState {
        commands: Vec<String>,
        input: Vec<u8>,
        expected_size: Option<usize>,
        contents: Vec<u8>,
        complete: bool,
        error: Option<String>,
    }

    struct ScpServer {
        state: Arc<Mutex<ScpServerState>>,
    }

    impl server::Handler for ScpServer {
        type Error = russh::Error;

        async fn auth_none(&mut self, _user: &str) -> Result<server::Auth, Self::Error> {
            Ok(server::Auth::Accept)
        }

        async fn channel_open_session(
            &mut self,
            _channel: Channel<server::Msg>,
            reply: server::ChannelOpenHandle,
            _session: &mut server::Session,
        ) -> Result<(), Self::Error> {
            reply.accept().await;
            Ok(())
        }

        async fn exec_request(
            &mut self,
            channel: ChannelId,
            data: &[u8],
            session: &mut server::Session,
        ) -> Result<(), Self::Error> {
            let command = String::from_utf8_lossy(data).into_owned();
            self.state
                .lock()
                .expect("SCP test state should not be poisoned")
                .commands
                .push(command.clone());
            session.channel_success(channel)?;
            if command.starts_with("scp -t ") {
                session.data(channel, vec![0])?;
            } else {
                if command.contains("printf 'exists\\n'") {
                    session.data(channel, b"missing\n".to_vec())?;
                }
                session.exit_status_request(channel, 0)?;
                session.eof(channel)?;
                session.close(channel)?;
            }
            Ok(())
        }

        async fn data(
            &mut self,
            channel: ChannelId,
            data: &[u8],
            session: &mut server::Session,
        ) -> Result<(), Self::Error> {
            let mut acknowledge_header = false;
            let mut finish = false;
            {
                let mut state = self
                    .state
                    .lock()
                    .expect("SCP test state should not be poisoned");
                state.input.extend_from_slice(data);
                if state.expected_size.is_none()
                    && let Some(newline) = state.input.iter().position(|byte| *byte == b'\n')
                {
                    let header = state.input.drain(..=newline).collect::<Vec<_>>();
                    let header = String::from_utf8_lossy(&header);
                    state.expected_size = header
                        .split_ascii_whitespace()
                        .nth(1)
                        .and_then(|size| size.parse().ok());
                    if state.expected_size.is_none() || !header.starts_with('C') {
                        state.error = Some(format!("invalid SCP header: {header}"));
                    }
                    acknowledge_header = true;
                }
                if let Some(expected_size) = state.expected_size
                    && state.input.len() > expected_size
                {
                    state.contents = state.input.drain(..expected_size).collect();
                    if state.input.pop() != Some(0) {
                        state.error = Some("SCP payload did not end with a null byte".into());
                    }
                    state.complete = true;
                    finish = true;
                }
            }

            if acknowledge_header {
                session.data(channel, vec![0])?;
            }
            if finish {
                session.data(channel, vec![0])?;
                session.exit_status_request(channel, 0)?;
                session.eof(channel)?;
                session.close(channel)?;
            }
            Ok(())
        }
    }

    async fn start_scp_server() -> (
        std::net::SocketAddr,
        russh::keys::PublicKey,
        JoinHandle<()>,
        Arc<Mutex<ScpServerState>>,
    ) {
        let state = Arc::new(Mutex::new(ScpServerState::default()));
        let handler_state = state.clone();
        let host_key = russh::keys::PrivateKey::random(&mut rng(), russh::keys::Algorithm::Ed25519)
            .expect("temporary host key");
        let public_key = host_key.public_key().clone();
        let config = Arc::new(server::Config {
            auth_rejection_time: Duration::ZERO,
            inactivity_timeout: None,
            keys: vec![host_key],
            ..Default::default()
        });
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("SCP test listener");
        let address = listener.local_addr().expect("SCP test address");
        let task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("SCP test connection");
            if let Ok(running) = server::run_stream(
                config,
                stream,
                ScpServer {
                    state: handler_state,
                },
            )
            .await
            {
                let _ = running.await;
            }
        });
        (address, public_key, task, state)
    }

    #[test]
    fn shell_paths_are_single_quoted_without_command_injection() {
        assert_eq!(
            shell_quote("/tmp/report's final").unwrap(),
            "'/tmp/report'\\''s final'"
        );
        assert_eq!(
            shell_quote("$(touch /tmp/pwned)").unwrap(),
            "'$(touch /tmp/pwned)'"
        );
    }

    #[test]
    fn scp_ack_parser_accepts_success_and_surfaces_remote_errors() {
        let mut success = VecDeque::from([0, 0]);
        assert!(take_scp_ack(&mut success).unwrap().is_ok());
        assert_eq!(success, VecDeque::from([0]));

        let mut failure = VecDeque::from(b"\x01permission denied\n".to_vec());
        let error = take_scp_ack(&mut failure).unwrap().unwrap_err();
        assert!(error.message().contains("permission denied"));
    }

    #[test]
    fn scp_ack_parser_waits_for_a_complete_error_line() {
        let mut response = VecDeque::from(b"\x01partial".to_vec());
        assert!(take_scp_ack(&mut response).is_none());
        response.extend(b" error\n");
        assert!(take_scp_ack(&mut response).unwrap().is_err());
    }

    #[test]
    fn scp_destinations_reject_protocol_line_breaks() {
        assert!(validate_remote_path("/tmp/good").is_ok());
        assert!(validate_remote_path("/tmp/bad\nname").is_err());
    }

    #[test]
    fn scp_completion_uses_the_existing_transfer_event_contract() {
        assert_eq!(
            transfer_result_event(
                7,
                "/tmp/report".into(),
                SftpTransferDirection::Upload,
                Ok(TransferResult::Completed(42)),
            ),
            ConnectionEvent::TransferCompleted {
                transfer_id: 7,
                direction: SftpTransferDirection::Upload,
                path: "/tmp/report".into(),
                bytes: 42,
            }
        );
    }

    #[tokio::test]
    async fn uploads_file_bytes_over_the_scp_sink_protocol() {
        let (address, public_key, server_task, state) = start_scp_server().await;
        let temporary = tempfile::tempdir().expect("temporary directory");
        let known_hosts = temporary.path().join("known_hosts");
        russh::keys::known_hosts::learn_known_hosts_path(
            "127.0.0.1",
            address.port(),
            &public_key,
            &known_hosts,
        )
        .expect("test host key should be recorded");
        let handler =
            ClientHandler::with_known_hosts_path("127.0.0.1", address.port(), known_hosts);
        let mut handle = client::connect(Arc::new(client::Config::default()), address, handler)
            .await
            .expect("SCP test connection");
        assert!(
            handle
                .authenticate_none("tester")
                .await
                .expect("SCP test authentication")
                .success()
        );
        let transport = SshTransport::from_test_handle(handle);
        let local_path = temporary.path().join("report.txt");
        fs::write(&local_path, b"SCP fallback payload")
            .await
            .expect("local SCP payload");
        let (events, mut event_rx) = mpsc::channel(8);
        let context = TransferContext::new(
            17,
            Arc::new(AtomicBool::new(false)),
            events,
            Arc::new(TransferRateLimiter::default()),
        );

        let result = upload_file(&transport, &local_path, "/tmp/report.txt", false, &context)
            .await
            .expect("SCP protocol upload");
        assert!(matches!(result, TransferResult::Completed(20)));

        assert_eq!(
            event_rx.recv().await,
            Some(ConnectionEvent::TransferProgress {
                transfer_id: 17,
                transferred: 20,
                total: Some(20),
            })
        );
        {
            let state = state.lock().expect("SCP test state should not be poisoned");
            assert_eq!(state.commands.len(), 4);
            assert!(state.commands[0].contains("printf 'exists\\n'"));
            assert!(state.commands[1].starts_with("rm -f -- "));
            assert!(state.commands[2].starts_with("scp -t -- '/tmp/report.txt.remcmd-"));
            assert!(state.commands[3].contains("mv -- '/tmp/report.txt.remcmd-"));
            assert_eq!(state.contents, b"SCP fallback payload");
            assert!(state.complete);
            assert_eq!(state.error, None);
        }

        transport.disconnect().await.expect("SCP test disconnect");
        timeout(Duration::from_secs(1), server_task)
            .await
            .expect("SCP test server should stop")
            .expect("SCP test server task");
    }
}
