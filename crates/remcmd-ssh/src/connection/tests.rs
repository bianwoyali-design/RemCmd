use std::time::Duration;

use super::*;

#[test]
fn connection_handle_forwards_commands() {
    let (command_tx, mut command_rx) = mpsc::unbounded_channel();
    let handle = ConnectionHandle { command_tx };
    let size = PtySize::new(120, 40);

    handle
        .send_input(b"pwd\n".to_vec())
        .expect("input should be sent");
    handle.resize(size).expect("resize should be sent");
    handle.disconnect().expect("disconnect should be sent");

    assert_eq!(
        command_rx.try_recv().expect("input command"),
        ConnectionCommand::Input(b"pwd\n".to_vec())
    );
    assert_eq!(
        command_rx.try_recv().expect("resize command"),
        ConnectionCommand::Resize(size)
    );
    assert_eq!(
        command_rx.try_recv().expect("disconnect command"),
        ConnectionCommand::Disconnect
    );
}

#[test]
fn queued_resizes_are_coalesced_without_reordering_other_commands() {
    let (command_tx, mut command_rx) = mpsc::unbounded_channel();
    let first_size = PtySize::new(90, 30);
    let latest_size = PtySize::new(120, 40);
    let later_size = PtySize::new(140, 50);

    command_tx
        .send(ConnectionCommand::Resize(first_size))
        .unwrap();
    command_tx
        .send(ConnectionCommand::Resize(latest_size))
        .unwrap();
    command_tx
        .send(ConnectionCommand::Input(b"pwd\n".to_vec()))
        .unwrap();
    command_tx
        .send(ConnectionCommand::Resize(later_size))
        .unwrap();

    let ConnectionCommand::Resize(initial_size) = command_rx.try_recv().unwrap() else {
        panic!("first command should be a resize");
    };
    let (coalesced_size, pending_command) = coalesce_queued_resizes(initial_size, &mut command_rx);

    assert_eq!(coalesced_size, latest_size);
    assert_eq!(
        pending_command,
        Some(ConnectionCommand::Input(b"pwd\n".to_vec()))
    );
    assert_eq!(
        command_rx.try_recv().unwrap(),
        ConnectionCommand::Resize(later_size)
    );
}

#[test]
fn closed_command_channel_returns_invalid_state_error() {
    let (command_tx, command_rx) = mpsc::unbounded_channel();
    let handle = ConnectionHandle { command_tx };

    drop(command_rx);

    let error = handle
        .disconnect()
        .expect_err("closed worker should reject commands");

    assert_eq!(error.kind(), SshErrorKind::InvalidState);
}

#[tokio::test]
async fn event_receiver_preserves_event_order() {
    let (event_tx, event_rx) = mpsc::channel(4);
    let mut receiver = ConnectionEventReceiver { event_rx };
    let resized = PtySize::new(120, 40);

    event_tx
        .send(ConnectionEvent::StateChanged(SessionState::Connecting))
        .await
        .expect("connecting event should be sent");
    event_tx
        .send(ConnectionEvent::StateChanged(SessionState::Authenticating))
        .await
        .expect("authenticating event should be sent");
    event_tx
        .send(ConnectionEvent::Resized(resized))
        .await
        .expect("resize confirmation should be sent");
    event_tx
        .send(ConnectionEvent::Shell(ShellEvent::Output(
            b"prompt".to_vec(),
        )))
        .await
        .expect("shell output should be sent");

    assert_eq!(
        receiver.next_event().await,
        Some(ConnectionEvent::StateChanged(SessionState::Connecting))
    );
    assert_eq!(
        receiver.next_event().await,
        Some(ConnectionEvent::StateChanged(SessionState::Authenticating))
    );
    assert_eq!(
        receiver.next_event().await,
        Some(ConnectionEvent::Resized(resized))
    );
    assert_eq!(
        receiver.next_event().await,
        Some(ConnectionEvent::Shell(ShellEvent::Output(
            b"prompt".to_vec()
        )))
    );
}

fn test_profile(port: u16) -> ConnectionProfile {
    ConnectionProfile::new("worker-test", "Worker Test", "127.0.0.1", port, "tester")
}

async fn next_event(receiver: &mut ConnectionEventReceiver) -> ConnectionEvent {
    tokio::time::timeout(Duration::from_secs(1), receiver.next_event())
        .await
        .expect("worker event should not time out")
        .expect("worker should still be running")
}

#[tokio::test]
async fn worker_reports_connection_failure() {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("temporary TCP port");
    let port = listener.local_addr().expect("local address").port();
    drop(listener);

    let connection = SshConnection::spawn(
        &Handle::current(),
        test_profile(port),
        AuthMethod::Agent,
        PtySize::default(),
    );
    let (_handle, mut events) = connection.split();

    assert_eq!(
        next_event(&mut events).await,
        ConnectionEvent::StateChanged(SessionState::Connecting)
    );

    let ConnectionEvent::Failed(error) = next_event(&mut events).await else {
        panic!("connection refusal should produce a failure event");
    };

    assert_eq!(error.kind(), SshErrorKind::Network);
}

#[tokio::test]
async fn worker_cancels_a_stalled_handshake() {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("local listener");
    let port = listener.local_addr().expect("local address").port();

    let server_task = tokio::spawn(async move {
        let (_stream, _) = listener.accept().await.expect("TCP connection");
        tokio::time::sleep(Duration::from_secs(2)).await;
    });

    let connection = SshConnection::spawn(
        &Handle::current(),
        test_profile(port),
        AuthMethod::Agent,
        PtySize::default(),
    );
    let (handle, mut events) = connection.split();

    assert_eq!(
        next_event(&mut events).await,
        ConnectionEvent::StateChanged(SessionState::Connecting)
    );

    handle
        .disconnect()
        .expect("disconnect command should be sent");

    assert_eq!(
        next_event(&mut events).await,
        ConnectionEvent::StateChanged(SessionState::Disconnecting)
    );
    assert_eq!(
        next_event(&mut events).await,
        ConnectionEvent::StateChanged(SessionState::Disconnected)
    );

    server_task.abort();
}
