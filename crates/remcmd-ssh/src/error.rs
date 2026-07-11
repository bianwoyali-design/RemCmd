use std::{error::Error, fmt};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SshErrorKind {
    InvalidState,
    Configuration,
    Network,
    HostKey,
    Authentication,
    Timeout,
    Protocol,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshError {
    kind: SshErrorKind,
    message: String,
}

impl SshError {
    pub fn new(kind: SshErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub const fn kind(&self) -> SshErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for SshError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for SshError {}

impl From<russh::Error> for SshError {
    fn from(error: russh::Error) -> Self {
        use russh::Error as RusshError;

        let kind = match &error {
            RusshError::UnknownKey
            | RusshError::WrongServerSig
            | RusshError::KeyChanged { .. }
            | RusshError::Keys(russh::keys::Error::KeyChanged { .. }) => SshErrorKind::HostKey,

            RusshError::NotAuthenticated
            | RusshError::UnsupportedAuthMethod
            | RusshError::NoAuthMethod => SshErrorKind::Authentication,

            RusshError::IO(_)
            | RusshError::HUP
            | RusshError::Disconnect
            | RusshError::SendError => SshErrorKind::Network,

            RusshError::ConnectionTimeout
            | RusshError::KeepaliveTimeout
            | RusshError::InactivityTimeout
            | RusshError::Elapsed(_) => SshErrorKind::Timeout,

            RusshError::NoHomeDir
            | RusshError::InvalidConfig(_)
            | RusshError::Keys(russh::keys::Error::NoHomeDir) => SshErrorKind::Configuration,

            _ => SshErrorKind::Protocol,
        };

        Self::new(kind, error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_exposes_kind_and_message() {
        let error = SshError::new(SshErrorKind::Network, "connection refused");

        assert_eq!(error.kind(), SshErrorKind::Network);
        assert_eq!(error.message(), "connection refused");
        assert_eq!(error.to_string(), "connection refused");
    }

    #[test]
    fn unknown_server_key_maps_to_host_key_error() {
        let error = SshError::from(russh::Error::UnknownKey);

        assert_eq!(error.kind(), SshErrorKind::HostKey);
    }

    #[test]
    fn io_error_maps_to_network_error() {
        let source =
            std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "connection refused");
        let error = SshError::from(russh::Error::IO(source));

        assert_eq!(error.kind(), SshErrorKind::Network);
        assert_eq!(error.message(), "connection refused");
    }

    #[test]
    fn connection_timeout_maps_to_timeout_error() {
        let error = SshError::from(russh::Error::ConnectionTimeout);

        assert_eq!(error.kind(), SshErrorKind::Timeout);
    }
}
