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
}
