use std::{
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

use crate::{
    AuthMethod, HostKeyInfo, PtySize, SshError, SshErrorKind, SshShell,
    shell_integration::ShellIntegration,
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const AUTHENTICATION_TIMEOUT: Duration = Duration::from_secs(10);
const SHELL_OPEN_TIMEOUT: Duration = Duration::from_secs(10);
const SHELL_DETECTION_TIMEOUT: Duration = Duration::from_secs(2);
const SFTP_OPEN_TIMEOUT: Duration = Duration::from_secs(10);
const SHELL_DETECTION_COMMAND: &str = "printf '%s' \"$SHELL\"";
const MAX_SHELL_PATH_BYTES: usize = 256;

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
}

impl SshTransport {
    /// Opens TCP and completes the SSH handshake without authenticating.
    async fn open_connection_with_timeout(
        profile: &ConnectionProfile,
        timeout: Duration,
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
        let connection = client::connect(config, (profile.host.as_str(), profile.port), handler);

        let result = tokio::time::timeout(timeout, connection)
            .await
            .map_err(|_| {
                SshError::new(
                    SshErrorKind::Timeout,
                    format!("connection to {}:{} timed out", profile.host, profile.port),
                )
            })?;

        match result {
            Ok(handle) => Ok(TransportOpen::Connected(Self { handle })),
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

    async fn authenticate_with_timeout(
        handle: &mut client::Handle<ClientHandler>,
        username: &str,
        auth: AuthMethod,
        timeout: Duration,
    ) -> Result<(), SshError> {
        match auth {
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

    /// Opens TCP and completes the SSH handshake without authenticating.
    ///
    /// This remains crate-private so callers outside remcmd-ssh cannot retain
    /// an unauthenticated transport accidentally.
    pub(crate) async fn open(profile: &ConnectionProfile) -> Result<TransportOpen, SshError> {
        Self::open_connection_with_timeout(profile, CONNECT_TIMEOUT).await
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
        let mut transport = match Self::open(profile).await? {
            TransportOpen::Connected(transport) => transport,
            TransportOpen::UnknownHostKey(pending) => return Err(pending.rejected_error()),
        };

        transport
            .authenticate(profile.username.as_str(), auth)
            .await?;

        Ok(transport)
    }

    pub async fn open_shell(&self, size: PtySize) -> Result<SshShell, SshError> {
        tokio::time::timeout(SHELL_OPEN_TIMEOUT, async {
            let shell_integration = self.detect_shell_integration().await;
            SshShell::open(&self.handle, size, shell_integration.as_ref()).await
        })
        .await
        .map_err(|_| SshError::new(SshErrorKind::Timeout, "opening remote shell timed out"))?
    }

    async fn detect_shell_integration(&self) -> Option<ShellIntegration> {
        tokio::time::timeout(SHELL_DETECTION_TIMEOUT, async {
            let mut channel = self.handle.channel_open_session().await.ok()?;
            channel.exec(true, SHELL_DETECTION_COMMAND).await.ok()?;

            let mut accepted = false;
            let mut succeeded = true;
            let mut output = Vec::new();
            while let Some(message) = channel.wait().await {
                match message {
                    ChannelMsg::Success => accepted = true,
                    ChannelMsg::Failure => return None,
                    ChannelMsg::Data { data } => {
                        let remaining = MAX_SHELL_PATH_BYTES.saturating_sub(output.len());
                        output.extend_from_slice(&data[..data.len().min(remaining)]);
                    }
                    ChannelMsg::ExitStatus { exit_status } => succeeded = exit_status == 0,
                    ChannelMsg::Eof | ChannelMsg::Close => break,
                    _ => {}
                }
            }

            (accepted && succeeded)
                .then(|| ShellIntegration::detect(&output))
                .flatten()
        })
        .await
        .ok()
        .flatten()
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

    /// Sends a protocol-level disconnect request to the server.
    ///
    /// Dropping SshTransport also closes local resources, but this method
    /// lets the server receive an explicit and orderly disconnect message.
    pub async fn disconnect(&self) -> Result<(), SshError> {
        self.handle
            .disconnect(
                russh::Disconnect::ByApplication,
                "Disconnected by user",
                "en",
            )
            .await
            .map_err(SshError::from)
    }

    /// Reports whether the russh background connection has stopped.
    pub fn is_closed(&self) -> bool {
        self.handle.is_closed()
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
