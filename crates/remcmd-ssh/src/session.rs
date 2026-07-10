use remcmd_core::ConnectionProfile;

use crate::{SshError, SshErrorKind};

/// Describes the current lifecycle stage of one SSH session.
///
/// The actual error is stored separately by `SshSession`, so this enum
/// remains small, copyable, and convenient for UI state checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SessionState {
    /// No connection exists and a new connection may be started.
    #[default]
    Disconnected,

    /// DNS lookup, TCP connection, and SSH handshake are in progress.
    Connecting,

    /// The server is connected and user authentication is in progress.
    Authenticating,

    /// Authentication succeeded and the session is ready.
    Connected,

    /// Resources are being closed.
    Disconnecting,

    /// The previous operation failed.
    Failed,
}

impl SessionState {
    /// Whether the Connect button should currently be enabled.
    pub const fn can_connect(self) -> bool {
        matches!(self, Self::Disconnected | Self::Failed)
    }

    /// Whether an active connection attempt or session can be stopped.
    pub const fn can_disconnect(self) -> bool {
        matches!(
            self,
            Self::Connecting | Self::Authenticating | Self::Connected
        )
    }
}

/// Represents one runtime SSH session.
///
/// It owns a snapshot of the connection profile. Editing the saved profile
/// later will not unexpectedly change an active session.
#[derive(Debug)]
pub struct SshSession {
    profile: ConnectionProfile,
    state: SessionState,
    last_error: Option<SshError>,
}

impl SshSession {
    /// Creates a session without opening a network connection.
    pub fn new(profile: ConnectionProfile) -> Self {
        Self {
            profile,
            state: SessionState::Disconnected,
            last_error: None,
        }
    }

    /// Returns the connection settings used by this session.
    pub fn profile(&self) -> &ConnectionProfile {
        &self.profile
    }

    /// Returns the current state by value because SessionState is Copy.
    pub const fn state(&self) -> SessionState {
        self.state
    }

    /// Returns the most recent failure without cloning its message.
    pub fn last_error(&self) -> Option<&SshError> {
        self.last_error.as_ref()
    }

    /// Starts a new connection attempt.
    ///
    /// Only disconnected or failed sessions may reconnect. Retrying clears
    /// the previous error because it belongs to the old attempt.
    pub fn begin_connect(&mut self) -> Result<(), SshError> {
        if !self.state.can_connect() {
            return Err(self.invalid_transition("start connecting"));
        }

        self.state = SessionState::Connecting;
        self.last_error = None;
        Ok(())
    }

    /// Moves from transport setup to user authentication.
    pub fn begin_authentication(&mut self) -> Result<(), SshError> {
        self.transition(
            SessionState::Connecting,
            SessionState::Authenticating,
            "start authentication",
        )
    }

    /// Marks authentication as successful.
    pub fn mark_connected(&mut self) -> Result<(), SshError> {
        self.transition(
            SessionState::Authenticating,
            SessionState::Connected,
            "finish authentication",
        )
    }

    /// Starts closing an active connection or connection attempt.
    pub fn begin_disconnect(&mut self) -> Result<(), SshError> {
        if !self.state.can_disconnect() {
            return Err(self.invalid_transition("start disconnecting"));
        }

        self.state = SessionState::Disconnecting;
        Ok(())
    }

    /// Marks resource cleanup as complete.
    pub fn mark_disconnected(&mut self) -> Result<(), SshError> {
        self.transition(
            SessionState::Disconnecting,
            SessionState::Disconnected,
            "finish disconnecting",
        )?;

        self.last_error = None;
        Ok(())
    }

    /// Records an operational failure from any connection stage.
    pub fn mark_failed(&mut self, error: SshError) {
        self.state = SessionState::Failed;
        self.last_error = Some(error);
    }

    /// Performs a transition that has exactly one valid source state.
    fn transition(
        &mut self,
        expected: SessionState,
        next: SessionState,
        operation: &str,
    ) -> Result<(), SshError> {
        if self.state != expected {
            return Err(self.invalid_transition(operation));
        }

        self.state = next;
        Ok(())
    }

    /// Creates a consistent error without changing the current state.
    fn invalid_transition(&self, operation: &str) -> SshError {
        SshError::new(
            SshErrorKind::InvalidState,
            format!("cannot {operation} while session is {:?}", self.state),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_state_is_disconnected() {
        assert_eq!(SessionState::default(), SessionState::Disconnected);
    }

    #[test]
    fn disconnected_and_failed_states_can_connect() {
        assert!(SessionState::Disconnected.can_connect());
        assert!(SessionState::Failed.can_connect());
        assert!(!SessionState::Connected.can_connect());
    }

    #[test]
    fn active_states_can_disconnect() {
        assert!(SessionState::Connecting.can_disconnect());
        assert!(SessionState::Authenticating.can_disconnect());
        assert!(SessionState::Connected.can_disconnect());
        assert!(!SessionState::Disconnected.can_disconnect());
        assert!(!SessionState::Disconnecting.can_disconnect());
        assert!(!SessionState::Failed.can_disconnect());
    }

    fn test_profile() -> ConnectionProfile {
        ConnectionProfile::new("test-profile", "Test Server", "127.0.0.1", 22, "tester")
    }

    #[test]
    fn new_session_starts_disconnected_without_an_error() {
        let session = SshSession::new(test_profile());

        assert_eq!(session.profile().id, "test-profile");
        assert_eq!(session.state(), SessionState::Disconnected);
        assert!(session.last_error().is_none());
    }

    #[test]
    fn session_follows_successful_connection_lifecycle() {
        let mut session = SshSession::new(test_profile());

        session.begin_connect().expect("connection should start");
        assert_eq!(session.state(), SessionState::Connecting);

        session
            .begin_authentication()
            .expect("authentication should start");
        assert_eq!(session.state(), SessionState::Authenticating);

        session
            .mark_connected()
            .expect("authentication should finish");
        assert_eq!(session.state(), SessionState::Connected);

        session
            .begin_disconnect()
            .expect("disconnection should start");
        assert_eq!(session.state(), SessionState::Disconnecting);

        session
            .mark_disconnected()
            .expect("disconnection should finish");
        assert_eq!(session.state(), SessionState::Disconnected);
    }

    #[test]
    fn invalid_transition_preserves_current_state() {
        let mut session = SshSession::new(test_profile());

        let error = session
            .mark_connected()
            .expect_err("invalid transition should fail");

        assert_eq!(error.kind(), SshErrorKind::InvalidState);
        assert_eq!(session.state(), SessionState::Disconnected);
        assert!(session.last_error().is_none());
    }

    #[test]
    fn retry_after_failure_clears_previous_error() {
        let mut session = SshSession::new(test_profile());

        session.begin_connect().expect("connection should start");
        session.mark_failed(SshError::new(SshErrorKind::Network, "connection refused"));

        assert_eq!(session.state(), SessionState::Failed);

        assert_eq!(
            session
                .last_error()
                .expect("failure should be stored")
                .message(),
            "connection refused"
        );

        session.begin_connect().expect("retry should start");

        assert_eq!(session.state(), SessionState::Connecting);
        assert!(session.last_error().is_none());
    }
}
