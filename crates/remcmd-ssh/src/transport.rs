use std::{path::PathBuf, sync::Arc, time::Duration};

use remcmd_core::ConnectionProfile;
use russh::{
    client,
    keys::{PublicKey, check_known_hosts, check_known_hosts_path},
};

use secrecy::ExposeSecret;

use crate::{AuthMethod, SshError, SshErrorKind};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const AUTHENTICATION_TIMEOUT: Duration = Duration::from_secs(10);

/// Receives asynchronous events from one russh client connection.
struct ClientHandler {
    host: String,
    port: u16,
    known_hosts_path: Option<PathBuf>,
}

impl ClientHandler {
    fn new(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
            known_hosts_path: None,
        }
    }

    /// Tests inject an isolated known_hosts file.
    #[cfg(test)]
    fn with_known_hosts_path(host: impl Into<String>, port: u16, path: PathBuf) -> Self {
        Self {
            host: host.into(),
            port,
            known_hosts_path: Some(path),
        }
    }

    fn verify_server_key(&self, server_public_key: &PublicKey) -> Result<bool, SshError> {
        let result = match &self.known_hosts_path {
            Some(path) => check_known_hosts_path(&self.host, self.port, server_public_key, path),
            None => check_known_hosts(&self.host, self.port, server_public_key),
        };

        result.map_err(russh::Error::from).map_err(SshError::from)
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
        self.verify_server_key(server_public_key)
    }
}

/// Owns the live russh connection after TCP and SSH handshakes complete.
pub struct SshTransport {
    handle: client::Handle<ClientHandler>,
}

impl SshTransport {
    /// Establishes and authenticates an SSH connection.
    ///
    /// AuthMethod is consumed so credentials are dropped after authentication.
    pub async fn connect(profile: &ConnectionProfile, auth: AuthMethod) -> Result<Self, SshError> {
        let mut handle = Self::open_connection_with_timeout(profile, CONNECT_TIMEOUT).await?;

        Self::authenticate_with_timeout(
            &mut handle,
            profile.username.as_str(),
            auth,
            AUTHENTICATION_TIMEOUT,
        )
        .await?;

        Ok(Self { handle })
    }

    /// Opens TCP and completes the SSH handshake without authenticating.
    async fn open_connection_with_timeout(
        profile: &ConnectionProfile,
        timeout: Duration,
    ) -> Result<client::Handle<ClientHandler>, SshError> {
        let config = Arc::new(client::Config {
            nodelay: true,
            ..Default::default()
        });

        let handler = ClientHandler::new(profile.host.clone(), profile.port);
        let connection = client::connect(config, (profile.host.as_str(), profile.port), handler);

        let handle = tokio::time::timeout(timeout, connection)
            .await
            .map_err(|_| {
                SshError::new(
                    SshErrorKind::Timeout,
                    format!("connection to {}:{} timed out", profile.host, profile.port),
                )
            })??;

        Ok(handle)
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

            AuthMethod::PrivateKey { .. } => Err(SshError::new(
                SshErrorKind::Authentication,
                "private-key authentication is not implemented yet",
            )),

            AuthMethod::Agent => Err(SshError::new(
                SshErrorKind::Authentication,
                "SSH Agent authentication is not implemented yet",
            )),
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

        assert_eq!(error.kind(), SshErrorKind::HostKey);
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
}
