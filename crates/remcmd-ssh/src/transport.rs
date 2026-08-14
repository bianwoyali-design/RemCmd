use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use directories::BaseDirs;
use remcmd_core::ConnectionProfile;
use russh::{
    ChannelMsg, client,
    keys::{
        Algorithm, PrivateKey, PrivateKeyWithHashAlg, PublicKey, check_known_hosts,
        check_known_hosts_path,
        known_hosts::{learn_known_hosts, learn_known_hosts_path},
    },
};

#[cfg(unix)]
use russh::keys::agent::{AgentIdentity, client::AgentClient};

use russh_sftp::client::SftpSession;
use secrecy::{ExposeSecret, SecretString};
use tokio::{
    net::{TcpStream, lookup_host},
    task::JoinSet,
};

use crate::{
    AuthMethod, ConnectionPlan, ConnectionStage, HostKeyInfo, PtySize, RuntimeProxy, SshError,
    SshErrorKind, SshShell,
    plan::ConnectionStep,
    proxy::{BoxedStream, open_initial_stream},
    shell_integration,
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const AUTHENTICATION_TIMEOUT: Duration = Duration::from_secs(10);
const SHELL_OPEN_TIMEOUT: Duration = Duration::from_secs(10);
const SFTP_OPEN_TIMEOUT: Duration = Duration::from_secs(10);
const EXEC_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_EXEC_OUTPUT_BYTES: usize = 64 * 1024;
const SFTP_AVAILABILITY_COMMAND: &str = r#"if command -v sftp-server >/dev/null 2>&1 || [ -x /usr/libexec/openssh/sftp-server ] || [ -x /usr/libexec/sftp-server ] || [ -x /usr/lib/openssh/sftp-server ] || [ -x /usr/lib/ssh/sftp-server ] || [ -x /usr/lib64/ssh/sftp-server ]; then printf 'available\n'; elif grep -Eqs '^[[:space:]]*Subsystem[[:space:]]+sftp[[:space:]]+internal-sftp([[:space:]]|$)' /etc/ssh/sshd_config 2>/dev/null || grep -ERqs '^[[:space:]]*Subsystem[[:space:]]+sftp[[:space:]]+internal-sftp([[:space:]]|$)' /etc/ssh/sshd_config.d 2>/dev/null; then printf 'available\n'; else printf 'unavailable\n'; fi"#;

/// Receives asynchronous events from one russh client connection.
struct ClientHandler {
    host: String,
    port: u16,
    known_hosts_path: Option<PathBuf>,
    unknown_server_key: Arc<Mutex<Option<PublicKey>>>,
}

impl ClientHandler {
    fn new(
        host: impl Into<String>,
        port: u16,
        unknown_server_key: Arc<Mutex<Option<PublicKey>>>,
    ) -> Self {
        Self {
            host: host.into(),
            port,
            known_hosts_path: None,
            unknown_server_key,
        }
    }

    /// Tests inject an isolated known_hosts file.
    #[cfg(test)]
    fn with_known_hosts_path(host: impl Into<String>, port: u16, path: PathBuf) -> Self {
        Self {
            host: host.into(),
            port,
            known_hosts_path: Some(path),
            unknown_server_key: Arc::default(),
        }
    }

    fn verify_server_key(&self, server_public_key: &PublicKey) -> Result<bool, SshError> {
        let result = match &self.known_hosts_path {
            Some(path) => check_known_hosts_path(&self.host, self.port, server_public_key, path),
            None => check_known_hosts(&self.host, self.port, server_public_key),
        };

        result.map_err(|error| match error {
            russh::keys::Error::KeyChanged { line } => SshError::new(
                SshErrorKind::HostKeyChanged,
                format!(
                    "the host key for {}:{} changed at known_hosts line {line}",
                    self.host, self.port
                ),
            ),
            error => SshError::from(russh::Error::Keys(error)),
        })
    }

    fn capture_unknown_server_key(&self, server_public_key: &PublicKey) -> Result<(), SshError> {
        let mut unknown_server_key = self.unknown_server_key.lock().map_err(|_| {
            SshError::new(
                SshErrorKind::Protocol,
                "host-key verification state is unavailable",
            )
        })?;
        *unknown_server_key = Some(server_public_key.clone());
        Ok(())
    }
}

impl client::Handler for ClientHandler {
    type Error = SshError;

    /// Accepts only keys already recorded in ~/.ssh/known_hosts.
    ///
    /// Unknown keys return false. Changed keys return an error. Neither case
    /// is accepted automatically because that would permit MITM attacks.
    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKey,
    ) -> Result<bool, Self::Error> {
        let is_known = self.verify_server_key(server_public_key)?;
        if !is_known {
            self.capture_unknown_server_key(server_public_key)?;
        }
        Ok(is_known)
    }
}

pub(crate) enum TransportOpen {
    Connected(SshTransport),
    UnknownHostKey(Box<PendingHostKey>),
}

pub(crate) struct PendingHostKey {
    info: HostKeyInfo,
    public_key: PublicKey,
    known_hosts_path: Option<PathBuf>,
}

impl PendingHostKey {
    fn new(
        host: String,
        port: u16,
        public_key: PublicKey,
        known_hosts_path: Option<PathBuf>,
    ) -> Self {
        let info = HostKeyInfo::from_public_key(host, port, &public_key);
        Self {
            info,
            public_key,
            known_hosts_path,
        }
    }

    pub(crate) fn info(&self) -> &HostKeyInfo {
        &self.info
    }

    pub(crate) fn rejected_error(&self) -> SshError {
        SshError::new(
            SshErrorKind::HostKeyUntrusted,
            format!("host key for {} was not trusted", self.info.address()),
        )
    }

    pub(crate) async fn trust(self) -> Result<(), SshError> {
        let info = self.info;
        let host = info.host().to_owned();
        let port = info.port();
        let public_key = self.public_key;
        let known_hosts_path = self.known_hosts_path;

        tokio::task::spawn_blocking(move || match known_hosts_path {
            Some(path) => learn_known_hosts_path(&host, port, &public_key, path),
            None => learn_known_hosts(&host, port, &public_key),
        })
        .await
        .map_err(|error| {
            SshError::new(
                SshErrorKind::HostKeyPersistence,
                format!("failed to record host key for {}: {error}", info.address()),
            )
        })?
        .map_err(|error| {
            SshError::new(
                SshErrorKind::HostKeyPersistence,
                format!("failed to record host key for {}: {error}", info.address()),
            )
        })
    }
}

/// Owns the live russh connection after TCP and SSH handshakes complete.
pub struct SshTransport {
    handle: client::Handle<ClientHandler>,
    upstream_handles: Vec<client::Handle<ClientHandler>>,
}

impl SshTransport {
    /// Opens TCP and completes the SSH handshake without authenticating.
    #[cfg(test)]
    async fn open_connection_with_timeout(
        profile: &ConnectionProfile,
        timeout: Duration,
    ) -> Result<TransportOpen, SshError> {
        Self::open_first_with_timeout(profile, None, timeout).await
    }

    async fn open_first_with_timeout(
        profile: &ConnectionProfile,
        proxy: Option<&RuntimeProxy>,
        timeout: Duration,
    ) -> Result<TransportOpen, SshError> {
        let connection = async {
            let stream = open_initial_stream(proxy, profile).await?;
            Self::open_stream(profile, stream).await
        };
        tokio::time::timeout(timeout, connection)
            .await
            .map_err(|_| {
                SshError::new(
                    SshErrorKind::Timeout,
                    format!("connection to {}:{} timed out", profile.host, profile.port),
                )
            })?
    }

    async fn open_stream(
        profile: &ConnectionProfile,
        stream: BoxedStream,
    ) -> Result<TransportOpen, SshError> {
        let config = Arc::new(client::Config {
            nodelay: true,
            ..Default::default()
        });

        let unknown_server_key = Arc::new(Mutex::new(None));
        let handler = ClientHandler::new(
            profile.host.clone(),
            profile.port,
            unknown_server_key.clone(),
        );
        let result = client::connect_stream(config, stream, handler).await;

        match result {
            Ok(handle) => Ok(TransportOpen::Connected(Self {
                handle,
                upstream_handles: Vec::new(),
            })),
            Err(error) => {
                if error.kind() != SshErrorKind::HostKeyUntrusted {
                    return Err(error);
                }

                let public_key = unknown_server_key
                    .lock()
                    .map_err(|_| {
                        SshError::new(
                            SshErrorKind::Protocol,
                            "host-key verification state is unavailable",
                        )
                    })?
                    .take()
                    .ok_or(error)?;

                Ok(TransportOpen::UnknownHostKey(Box::new(
                    PendingHostKey::new(profile.host.clone(), profile.port, public_key, None),
                )))
            }
        }
    }

    async fn open_via_with_timeout(
        &self,
        profile: &ConnectionProfile,
        timeout: Duration,
    ) -> Result<TransportOpen, SshError> {
        let connection = async {
            let channel = self
                .handle
                .channel_open_direct_tcpip(&profile.host, u32::from(profile.port), "127.0.0.1", 0)
                .await
                .map_err(SshError::from)?;
            Self::open_stream(profile, Box::new(channel.into_stream())).await
        };
        tokio::time::timeout(timeout, connection)
            .await
            .map_err(|_| {
                SshError::new(
                    SshErrorKind::Timeout,
                    format!(
                        "opening tunneled connection to {}:{} timed out",
                        profile.host, profile.port
                    ),
                )
            })?
    }

    async fn authenticate_with_timeout(
        handle: &mut client::Handle<ClientHandler>,
        username: &str,
        auth: AuthMethod,
        timeout: Duration,
    ) -> Result<(), SshError> {
        match auth {
            AuthMethod::None => {
                let authentication = handle.authenticate_none(username);
                let result = tokio::time::timeout(timeout, authentication)
                    .await
                    .map_err(|_| {
                        SshError::new(
                            SshErrorKind::Timeout,
                            format!("authentication for user {username} timed out"),
                        )
                    })?
                    .map_err(SshError::from)?;

                Self::validate_authentication_result(result, username)
            }

            AuthMethod::Password { password } => {
                // Reading SecretString requires an explicit ExposeSecret call.
                let authentication =
                    handle.authenticate_password(username, password.expose_secret());

                let result = tokio::time::timeout(timeout, authentication)
                    .await
                    .map_err(|_| {
                        SshError::new(
                            SshErrorKind::Timeout,
                            format!("authentication for user {username} timed out"),
                        )
                    })?
                    .map_err(SshError::from)?;

                Self::validate_authentication_result(result, username)
            }

            AuthMethod::PrivateKey { path, passphrase } => {
                let private_key = Self::load_private_key(path, passphrase).await?;

                let hash_algorithm = if matches!(private_key.algorithm(), Algorithm::Rsa { .. }) {
                    handle
                        .best_supported_rsa_hash()
                        .await
                        .map_err(SshError::from)?
                        .flatten()
                } else {
                    None
                };

                let private_key = PrivateKeyWithHashAlg::new(Arc::new(private_key), hash_algorithm);

                let authentication = handle.authenticate_publickey(username, private_key);

                let result = tokio::time::timeout(timeout, authentication)
                    .await
                    .map_err(|_| {
                        SshError::new(
                            SshErrorKind::Timeout,
                            format!("authentication for user {username} timed out"),
                        )
                    })?
                    .map_err(SshError::from)?;

                Self::validate_authentication_result(result, username)
            }

            AuthMethod::Agent => {
                // Apply one timeout to connecting, listing keys, signing,
                // and waiting for the server's authentication response.
                let authentication = Self::authenticate_with_agent(handle, username);

                tokio::time::timeout(timeout, authentication)
                    .await
                    .map_err(|_| {
                        SshError::new(
                            SshErrorKind::Timeout,
                            format!("authentication for user {username} timed out"),
                        )
                    })?
            }
        }
    }

    fn validate_authentication_result(
        result: client::AuthResult,
        username: &str,
    ) -> Result<(), SshError> {
        if result.success() {
            return Ok(());
        }

        Err(SshError::new(
            SshErrorKind::Authentication,
            format!("authentication failed for user {username}"),
        ))
    }

    /// Loads and optionally decrypts a private key outside Tokio's async workers.
    async fn load_private_key(
        path: PathBuf,
        passphrase: Option<SecretString>,
    ) -> Result<PrivateKey, SshError> {
        let base_dirs = BaseDirs::new();
        let path = Self::expand_home_path(
            &path,
            base_dirs.as_ref().map(|base_dirs| base_dirs.home_dir()),
        )?;

        // Keep a copy for the error message because the original path
        // is moved into the blocking task.
        let error_path = path.clone();

        let result = tokio::task::spawn_blocking(move || {
            let passphrase = passphrase
                .as_ref()
                .map(|passphrase| passphrase.expose_secret());

            russh::keys::load_secret_key(path, passphrase)
        })
        .await
        .map_err(|error| {
            SshError::new(
                SshErrorKind::Protocol,
                format!("private-key loader task failed: {error}"),
            )
        })?;

        result.map_err(|error| Self::private_key_load_error(&error_path, error))
    }

    fn expand_home_path(path: &Path, home_dir: Option<&Path>) -> Result<PathBuf, SshError> {
        let Ok(relative_path) = path.strip_prefix("~") else {
            return Ok(path.to_path_buf());
        };

        let home_dir = home_dir.ok_or_else(|| {
            SshError::new(
                SshErrorKind::Configuration,
                "cannot expand private-key path because the home directory is unavailable",
            )
        })?;

        Ok(home_dir.join(relative_path))
    }

    /// Converts key-file and decryption failures into application errors.
    fn private_key_load_error(path: &Path, error: russh::keys::Error) -> SshError {
        let kind = match &error {
            // These errors generally indicate a missing or incorrect passphrase.
            russh::keys::Error::KeyIsEncrypted
            | russh::keys::Error::Pad(_)
            | russh::keys::Error::Unpad(_)
            | russh::keys::Error::SshKey(russh::keys::ssh_key::Error::Crypto) => {
                SshErrorKind::PrivateKeyPassphrase
            }

            // Missing, unreadable, corrupt, or unsupported files are configuration errors.
            _ => SshErrorKind::Configuration,
        };

        SshError::new(
            kind,
            format!("failed to load private key {}: {error}", path.display()),
        )
    }

    #[cfg(unix)]
    async fn authenticate_with_agent(
        handle: &mut client::Handle<ClientHandler>,
        username: &str,
    ) -> Result<(), SshError> {
        // Connect to the Unix socket specified by SSH_AUTH_SOCK.
        let mut agent = AgentClient::connect_env().await.map_err(|error| {
            SshError::new(
                SshErrorKind::Configuration,
                format!("failed to connect to SSH Agent: {error}"),
            )
        })?;

        let identities = agent.request_identities().await.map_err(|error| {
            SshError::new(
                SshErrorKind::Authentication,
                format!("failed to list SSH Agent identities: {error}"),
            )
        })?;

        if identities.is_empty() {
            return Err(SshError::new(
                SshErrorKind::Authentication,
                "SSH Agent contains no identities",
            ));
        }

        // Outer Option records whether RSA negotiation has already run.
        // Inner Option is the hash algorithm returned by the server.
        let mut cached_rsa_hash = None;

        for identity in identities {
            let is_rsa = matches!(identity.public_key().algorithm(), Algorithm::Rsa { .. });

            let hash_algorithm = if is_rsa {
                match cached_rsa_hash {
                    Some(hash_algorithm) => hash_algorithm,
                    None => {
                        let hash_algorithm = handle
                            .best_supported_rsa_hash()
                            .await
                            .map_err(SshError::from)?
                            .flatten();

                        cached_rsa_hash = Some(hash_algorithm);
                        hash_algorithm
                    }
                }
            } else {
                None
            };

            let result = match identity {
                AgentIdentity::PublicKey { key, .. } => {
                    handle
                        .authenticate_publickey_with(username, key, hash_algorithm, &mut agent)
                        .await
                }

                AgentIdentity::Certificate { certificate, .. } => {
                    handle
                        .authenticate_certificate_with(
                            username,
                            certificate,
                            hash_algorithm,
                            &mut agent,
                        )
                        .await
                }
            }
            .map_err(|error| {
                SshError::new(
                    SshErrorKind::Authentication,
                    format!("SSH Agent signing failed: {error}"),
                )
            })?;

            // Servers may reject one key but accept another, so continue trying.
            if result.success() {
                return Ok(());
            }
        }

        Err(SshError::new(
            SshErrorKind::Authentication,
            format!("SSH Agent has no key accepted for user {username}"),
        ))
    }

    #[cfg(not(unix))]
    async fn authenticate_with_agent(
        _handle: &mut client::Handle<ClientHandler>,
        _username: &str,
    ) -> Result<(), SshError> {
        Err(SshError::new(
            SshErrorKind::Configuration,
            "SSH Agent authentication is not supported on this platform",
        ))
    }

    pub(crate) async fn open_first(
        profile: &ConnectionProfile,
        proxy: Option<&RuntimeProxy>,
    ) -> Result<TransportOpen, SshError> {
        Self::open_first_with_timeout(profile, proxy, CONNECT_TIMEOUT).await
    }

    pub(crate) async fn open_via(
        &self,
        profile: &ConnectionProfile,
    ) -> Result<TransportOpen, SshError> {
        self.open_via_with_timeout(profile, CONNECT_TIMEOUT).await
    }

    /// Authenticates an already-open SSH transport.
    ///
    /// AuthMethod is consumed so credentials are dropped after this attempt.
    pub(crate) async fn authenticate(
        &mut self,
        username: &str,
        auth: AuthMethod,
    ) -> Result<(), SshError> {
        Self::authenticate_with_timeout(&mut self.handle, username, auth, AUTHENTICATION_TIMEOUT)
            .await
    }

    /// Establishes and authenticates an SSH connection.
    ///
    /// This convenience API remains available to callers that do not need
    /// progress events for the individual connection stages.
    pub async fn connect(profile: &ConnectionProfile, auth: AuthMethod) -> Result<Self, SshError> {
        Self::connect_plan(ConnectionPlan::direct(profile.clone(), auth)).await
    }

    /// Establishes every proxy, jump, and target step in a validated runtime plan.
    pub async fn connect_plan(plan: ConnectionPlan) -> Result<Self, SshError> {
        plan.validate()?;
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
        let target_stage = ConnectionStage::Target {
            profile_id: target.profile.id.clone(),
        };
        steps.push((target, target_stage));

        let mut established: Vec<Self> = Vec::new();
        for (step_index, (step, stage)) in steps.into_iter().enumerate() {
            let ConnectionStep { profile, auth } = step;
            let opened = if step_index == 0 {
                Self::open_first(&profile, proxy.as_ref()).await
            } else {
                established
                    .last()
                    .expect("a previous jump transport exists")
                    .open_via(&profile)
                    .await
            };
            let mut transport = match opened {
                Ok(TransportOpen::Connected(transport)) => transport,
                Ok(TransportOpen::UnknownHostKey(pending)) => {
                    Self::disconnect_established(&established).await;
                    return Err(pending.rejected_error().at_stage(stage));
                }
                Err(error) => {
                    Self::disconnect_established(&established).await;
                    return Err(error.at_stage(stage));
                }
            };
            let auth_kind = auth.kind();
            if let Err(error) = transport.authenticate(&profile.username, auth).await {
                let _ = transport.disconnect_current().await;
                Self::disconnect_established(&established).await;
                tracing::warn!(
                    stage = stage_name(&stage),
                    authentication = ?auth_kind,
                    result = "failed",
                    "SSH authentication failed"
                );
                return Err(error.at_stage(stage));
            }
            tracing::info!(
                stage = stage_name(&stage),
                authentication = ?auth_kind,
                result = "success",
                "SSH authentication completed"
            );
            established.push(transport);
        }

        Ok(Self::combine_chain(established))
    }

    pub async fn open_shell(&self, size: PtySize) -> Result<SshShell, SshError> {
        tokio::time::timeout(
            SHELL_OPEN_TIMEOUT,
            SshShell::open(&self.handle, size, false),
        )
        .await
        .map_err(|_| SshError::new(SshErrorKind::Timeout, "opening remote shell timed out"))?
    }

    pub(crate) async fn detect_shell(
        &self,
    ) -> Result<Option<shell_integration::ShellKind>, SshError> {
        let output = self
            .execute(shell_integration::DETECT_SHELL_COMMAND)
            .await?;
        Ok(shell_integration::detect_shell(&output))
    }

    pub(crate) async fn open_sftp(&self) -> Result<SftpSession, SshError> {
        tokio::time::timeout(SFTP_OPEN_TIMEOUT, async {
            let channel = self
                .handle
                .channel_open_session()
                .await
                .map_err(SshError::from)?;
            channel
                .request_subsystem(true, "sftp")
                .await
                .map_err(SshError::from)?;
            SftpSession::new(channel.into_stream())
                .await
                .map_err(SshError::from)
        })
        .await
        .map_err(|_| SshError::new(SshErrorKind::Timeout, "opening SFTP timed out"))?
    }

    pub(crate) async fn check_sftp_availability(&self) -> Result<bool, SshError> {
        let output = self.execute(SFTP_AVAILABILITY_COMMAND).await?;
        parse_sftp_availability(&output)
    }

    pub(crate) async fn execute(&self, command: &str) -> Result<Vec<u8>, SshError> {
        tokio::time::timeout(EXEC_TIMEOUT, async {
            let mut channel = self
                .handle
                .channel_open_session()
                .await
                .map_err(SshError::from)?;
            channel
                .exec(true, command.as_bytes())
                .await
                .map_err(SshError::from)?;

            let mut accepted = false;
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let mut exit_status = None;
            while let Some(message) = channel.wait().await {
                match message {
                    ChannelMsg::Success => accepted = true,
                    ChannelMsg::Failure => {
                        return Err(SshError::new(
                            SshErrorKind::Protocol,
                            "remote server rejected command",
                        ));
                    }
                    ChannelMsg::Data { data } => {
                        append_command_output(&mut stdout, &data)?;
                    }
                    ChannelMsg::ExtendedData { data, .. } => {
                        append_command_output(&mut stderr, &data)?;
                    }
                    ChannelMsg::ExitStatus {
                        exit_status: status,
                    } => exit_status = Some(status),
                    ChannelMsg::Close => break,
                    ChannelMsg::Eof
                    | ChannelMsg::ExitSignal { .. }
                    | ChannelMsg::WindowAdjusted { .. } => {}
                    _ => {}
                }
            }

            if !accepted {
                return Err(SshError::new(
                    SshErrorKind::Protocol,
                    "remote server did not accept command",
                ));
            }
            if exit_status.unwrap_or(0) != 0 {
                let message = String::from_utf8_lossy(&stderr).trim().to_owned();
                return Err(SshError::new(
                    SshErrorKind::Protocol,
                    if message.is_empty() {
                        "remote command failed".to_owned()
                    } else {
                        message
                    },
                ));
            }

            Ok(stdout)
        })
        .await
        .map_err(|_| SshError::new(SshErrorKind::Timeout, "remote command timed out"))?
    }

    /// Sends a protocol-level disconnect request to the server.
    ///
    /// Dropping SshTransport also closes local resources, but this method
    /// lets the server receive an explicit and orderly disconnect message.
    pub async fn disconnect(&self) -> Result<(), SshError> {
        let mut first_error = self.disconnect_current().await.err();
        for handle in self.upstream_handles.iter().rev() {
            if let Err(error) = handle
                .disconnect(
                    russh::Disconnect::ByApplication,
                    "Disconnected by user",
                    "en",
                )
                .await
                .map_err(SshError::from)
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    async fn disconnect_current(&self) -> Result<(), SshError> {
        self.handle
            .disconnect(
                russh::Disconnect::ByApplication,
                "Disconnected by user",
                "en",
            )
            .await
            .map_err(SshError::from)
    }

    pub(crate) async fn disconnect_established(transports: &[Self]) {
        for transport in transports.iter().rev() {
            let _ = transport.disconnect_current().await;
        }
    }

    pub(crate) fn combine_chain(mut transports: Vec<Self>) -> Self {
        let mut final_transport = transports
            .pop()
            .expect("an SSH transport chain always contains a target");
        let mut upstream_handles = Vec::new();
        for mut transport in transports {
            upstream_handles.append(&mut transport.upstream_handles);
            upstream_handles.push(transport.handle);
        }
        upstream_handles.append(&mut final_transport.upstream_handles);
        final_transport.upstream_handles = upstream_handles;
        final_transport
    }

    /// Reports whether the russh background connection has stopped.
    pub fn is_closed(&self) -> bool {
        self.handle.is_closed()
    }
}

pub(crate) async fn connect_tcp(host: &str, port: u16) -> Result<TcpStream, SshError> {
    let addresses = lookup_host((host, port)).await.map_err(|error| {
        SshError::new(
            SshErrorKind::Network,
            format!("failed to resolve {host}:{port}: {error}"),
        )
    })?;
    race_tcp_connections(addresses).await
}

fn stage_name(stage: &ConnectionStage) -> &'static str {
    match stage {
        ConnectionStage::Proxy => "proxy",
        ConnectionStage::Jump { .. } => "jump",
        ConnectionStage::Target { .. } => "target",
    }
}

async fn race_tcp_connections(
    addresses: impl IntoIterator<Item = SocketAddr>,
) -> Result<TcpStream, SshError> {
    let mut attempts = JoinSet::new();
    let mut unique_addresses = Vec::new();
    for address in addresses {
        if !unique_addresses.contains(&address) {
            unique_addresses.push(address);
            attempts.spawn(async move { (address, TcpStream::connect(address).await) });
        }
    }

    if unique_addresses.is_empty() {
        return Err(SshError::new(
            SshErrorKind::Network,
            "the SSH host did not resolve to an address",
        ));
    }

    let mut failures = Vec::new();
    while let Some(result) = attempts.join_next().await {
        match result {
            Ok((_, Ok(stream))) => {
                attempts.abort_all();
                return Ok(stream);
            }
            Ok((address, Err(error))) => failures.push(format!("{address}: {error}")),
            Err(error) => failures.push(format!("connection attempt failed: {error}")),
        }
    }

    Err(SshError::new(
        SshErrorKind::Network,
        format!("all SSH addresses failed: {}", failures.join("; ")),
    ))
}

fn append_command_output(output: &mut Vec<u8>, data: &[u8]) -> Result<(), SshError> {
    if output.len().saturating_add(data.len()) > MAX_EXEC_OUTPUT_BYTES {
        return Err(SshError::new(
            SshErrorKind::Protocol,
            "remote performance response exceeded the output limit",
        ));
    }
    output.extend_from_slice(data);
    Ok(())
}

fn parse_sftp_availability(output: &[u8]) -> Result<bool, SshError> {
    match output {
        b"available\n" => Ok(true),
        b"unavailable\n" => Ok(false),
        _ => Err(SshError::new(
            SshErrorKind::Protocol,
            "remote server returned an invalid SFTP availability response",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PUBLIC_KEY: &str = "AAAAC3NzaC1lZDI1NTE5AAAAIJdD7y3aLq454yWBdwLWbieU1ebz9/cu7/QEXn9OIeZJ";
    const DIFFERENT_PUBLIC_KEY: &str =
        "AAAAC3NzaC1lZDI1NTE5AAAAILIG2T/B0l0gaqj3puu510tu9N1OkQ4znY3LYuEm5zCF";

    fn test_profile(port: u16) -> ConnectionProfile {
        ConnectionProfile::new("local-test", "Local Test", "127.0.0.1", port, "tester")
    }

    #[test]
    fn matching_known_host_key_is_accepted() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("known_hosts");

        std::fs::write(
            &path,
            format!("[localhost]:13265 ssh-ed25519 {PUBLIC_KEY}\n"),
        )
        .expect("known_hosts should be written");

        let public_key =
            russh::keys::parse_public_key_base64(PUBLIC_KEY).expect("public key should parse");

        let handler = ClientHandler::with_known_hosts_path("localhost", 13265, path);

        assert!(
            handler
                .verify_server_key(&public_key)
                .expect("verification should succeed")
        );
    }

    #[test]
    fn unknown_host_key_is_rejected() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("known_hosts");

        // The file does not exist, so no key is trusted for this endpoint.
        let public_key =
            russh::keys::parse_public_key_base64(PUBLIC_KEY).expect("public key should parse");

        let handler = ClientHandler::with_known_hosts_path("localhost", 13265, path);

        assert!(
            !handler
                .verify_server_key(&public_key)
                .expect("unknown key should not cause an IO error")
        );
    }

    #[tokio::test]
    async fn unknown_host_key_is_captured_for_explicit_review() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("known_hosts");
        let public_key =
            russh::keys::parse_public_key_base64(PUBLIC_KEY).expect("public key should parse");
        let mut handler = ClientHandler::with_known_hosts_path("localhost", 13265, path);

        let accepted = client::Handler::check_server_key(&mut handler, &public_key)
            .await
            .expect("unknown key should not cause an IO error");

        assert!(!accepted);
        assert_eq!(
            handler
                .unknown_server_key
                .lock()
                .expect("captured key lock")
                .as_ref(),
            Some(&public_key)
        );
    }

    #[test]
    fn changed_host_key_returns_host_key_error() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("known_hosts");

        // The recorded key and presented key use the same algorithm but differ.
        std::fs::write(
            &path,
            format!("[localhost]:13265 ssh-ed25519 {PUBLIC_KEY}\n"),
        )
        .expect("known_hosts should be written");

        let changed_key = russh::keys::parse_public_key_base64(DIFFERENT_PUBLIC_KEY)
            .expect("changed public key should parse");

        let handler = ClientHandler::with_known_hosts_path("localhost", 13265, path);

        let error = handler
            .verify_server_key(&changed_key)
            .expect_err("changed key must be rejected");

        assert_eq!(error.kind(), SshErrorKind::HostKeyChanged);
    }

    #[tokio::test]
    async fn trusting_unknown_host_key_records_exact_presented_key() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("nested").join("known_hosts");
        let public_key =
            russh::keys::parse_public_key_base64(PUBLIC_KEY).expect("public key should parse");
        let pending = PendingHostKey::new(
            "localhost".into(),
            13265,
            public_key.clone(),
            Some(path.clone()),
        );

        pending.trust().await.expect("host key should be recorded");

        assert!(
            check_known_hosts_path("localhost", 13265, &public_key, path)
                .expect("recorded key should be readable")
        );
    }

    #[tokio::test]
    async fn host_key_write_failure_is_typed() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let parent_file = directory.path().join("not-a-directory");
        std::fs::write(&parent_file, b"occupied").expect("parent file should be written");
        let public_key =
            russh::keys::parse_public_key_base64(PUBLIC_KEY).expect("public key should parse");
        let pending = PendingHostKey::new(
            "localhost".into(),
            13265,
            public_key,
            Some(parent_file.join("known_hosts")),
        );

        let error = pending
            .trust()
            .await
            .expect_err("invalid parent path should fail");

        assert_eq!(error.kind(), SshErrorKind::HostKeyPersistence);
    }

    #[test]
    fn rejecting_unknown_host_key_is_typed() {
        let public_key =
            russh::keys::parse_public_key_base64(PUBLIC_KEY).expect("public key should parse");
        let pending = PendingHostKey::new("localhost".into(), 22, public_key, None);

        let error = pending.rejected_error();

        assert_eq!(error.kind(), SshErrorKind::HostKeyUntrusted);
        assert!(error.message().contains("localhost:22"));
    }

    #[tokio::test]
    async fn tcp_connection_uses_the_first_reachable_resolved_address() {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("reachable listener");
        let reachable = listener.local_addr().expect("reachable address");
        let refused_listener =
            std::net::TcpListener::bind(("127.0.0.1", 0)).expect("temporary refused address");
        let refused = refused_listener.local_addr().expect("refused address");
        drop(refused_listener);

        let stream = race_tcp_connections([refused, reachable])
            .await
            .expect("one resolved address should connect");

        assert_eq!(stream.peer_addr().expect("peer address"), reachable);
    }

    #[tokio::test]
    async fn refused_tcp_connection_maps_to_network_error() {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("temporary TCP port");
        let port = listener.local_addr().expect("local address").port();

        // Closing the listener makes the selected port reject connections.
        drop(listener);

        let result =
            SshTransport::open_connection_with_timeout(&test_profile(port), Duration::from_secs(1))
                .await;

        let Err(error) = result else {
            panic!("connection should have been refused");
        };

        assert_eq!(error.kind(), SshErrorKind::Network);
    }

    #[tokio::test]
    async fn stalled_ssh_handshake_maps_to_timeout_error() {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("local listener");
        let port = listener.local_addr().expect("local address").port();

        // Accept TCP but deliberately never send an SSH identification string.
        let server_task = tokio::spawn(async move {
            let (_stream, _) = listener.accept().await.expect("TCP connection");
            tokio::time::sleep(Duration::from_secs(1)).await;
        });

        let result = SshTransport::open_connection_with_timeout(
            &test_profile(port),
            Duration::from_millis(50),
        )
        .await;

        server_task.abort();

        let Err(error) = result else {
            panic!("SSH handshake should have timed out");
        };

        assert_eq!(error.kind(), SshErrorKind::Timeout);
    }

    #[test]
    fn successful_authentication_result_is_accepted() {
        let result =
            SshTransport::validate_authentication_result(client::AuthResult::Success, "tester");

        assert!(result.is_ok());
    }

    #[test]
    fn rejected_authentication_maps_to_authentication_error() {
        let result = client::AuthResult::Failure {
            remaining_methods: russh::MethodSet::empty(),
            partial_success: false,
        };

        let error = SshTransport::validate_authentication_result(result, "tester")
            .expect_err("authentication should be rejected");

        assert_eq!(error.kind(), SshErrorKind::Authentication);
        assert_eq!(error.message(), "authentication failed for user tester");
    }

    #[test]
    fn parses_sftp_availability_probe_responses() {
        assert!(parse_sftp_availability(b"available\n").unwrap());
        assert!(!parse_sftp_availability(b"unavailable\n").unwrap());

        let error = parse_sftp_availability(b"unexpected\n").unwrap_err();
        assert_eq!(error.kind(), SshErrorKind::Protocol);
    }

    #[test]
    fn sftp_probe_covers_common_server_layouts() {
        for path in [
            "/usr/libexec/openssh/sftp-server",
            "/usr/libexec/sftp-server",
            "/usr/lib/openssh/sftp-server",
            "/usr/lib/ssh/sftp-server",
            "/usr/lib64/ssh/sftp-server",
        ] {
            assert!(SFTP_AVAILABILITY_COMMAND.contains(path));
        }
        assert!(SFTP_AVAILABILITY_COMMAND.contains("/etc/ssh/sshd_config.d"));
        assert!(!SFTP_AVAILABILITY_COMMAND.contains("sshd_config.d/*.conf"));
    }

    #[tokio::test]
    async fn missing_private_key_maps_to_configuration_error() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("missing-key");

        let result = SshTransport::load_private_key(path, None).await;

        let Err(error) = result else {
            panic!("missing private key should fail");
        };

        assert_eq!(error.kind(), SshErrorKind::Configuration);
    }

    #[test]
    fn private_key_path_expands_home_directory() {
        assert_eq!(
            SshTransport::expand_home_path(
                Path::new("~/.ssh/id_ed25519"),
                Some(Path::new("/Users/test")),
            )
            .expect("home-relative path should expand"),
            PathBuf::from("/Users/test/.ssh/id_ed25519")
        );
        assert_eq!(
            SshTransport::expand_home_path(Path::new("~"), Some(Path::new("/Users/test")))
                .expect("home path should expand"),
            PathBuf::from("/Users/test")
        );
    }

    #[test]
    fn private_key_path_only_expands_a_standalone_tilde_component() {
        for path in [
            Path::new("/tmp/id_ed25519"),
            Path::new(".ssh/id_ed25519"),
            Path::new("~other/.ssh/id_ed25519"),
        ] {
            assert_eq!(
                SshTransport::expand_home_path(path, None)
                    .expect("non-home-relative path should remain unchanged"),
                path
            );
        }
    }

    #[test]
    fn home_relative_private_key_path_requires_a_home_directory() {
        let error = SshTransport::expand_home_path(Path::new("~/.ssh/id_ed25519"), None)
            .expect_err("home-relative path should require a home directory");

        assert_eq!(error.kind(), SshErrorKind::Configuration);
    }

    #[test]
    fn encrypted_private_key_requires_passphrase() {
        let error = SshTransport::private_key_load_error(
            Path::new("/tmp/encrypted-key"),
            russh::keys::Error::KeyIsEncrypted,
        );

        assert_eq!(error.kind(), SshErrorKind::PrivateKeyPassphrase);
    }
}
