use crate::{SshError, SshErrorKind};

/// Describes the current lifecycle stage of one SSH session.
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

/// Validates lifecycle transitions while connection plans and events own their data.
#[derive(Debug, Default)]
pub struct SshSession {
    state: SessionState,
}

impl SshSession {
    /// Returns the current state by value because SessionState is Copy.
    pub const fn state(&self) -> SessionState {
        self.state
    }

    /// Starts a new connection attempt.
    ///
    /// Only disconnected or failed sessions may reconnect.
    pub fn begin_connect(&mut self) -> Result<(), SshError> {
        if !self.state.can_connect() {
            return Err(self.invalid_transition("start connecting"));
        }

        self.state = SessionState::Connecting;
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
        )
    }

    /// Marks an operational failure from any connection stage.
    pub fn mark_failed(&mut self) {
        self.state = SessionState::Failed;
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

    #[test]
    fn new_session_starts_disconnected() {
        let session = SshSession::default();

        assert_eq!(session.state(), SessionState::Disconnected);
    }

    #[test]
    fn session_follows_successful_connection_lifecycle() {
        let mut session = SshSession::default();

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
        let mut session = SshSession::default();

        let error = session
            .mark_connected()
            .expect_err("invalid transition should fail");

        assert_eq!(error.kind(), SshErrorKind::InvalidState);
        assert_eq!(session.state(), SessionState::Disconnected);
    }

    #[test]
    fn failed_session_can_retry() {
        let mut session = SshSession::default();

        session.begin_connect().expect("connection should start");
        session.mark_failed();

        assert_eq!(session.state(), SessionState::Failed);

        session.begin_connect().expect("retry should start");

        assert_eq!(session.state(), SessionState::Connecting);
    }
}
