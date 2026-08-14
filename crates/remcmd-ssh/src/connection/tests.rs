use std::time::Duration;

use super::*;

#[test]
fn connection_handle_forwards_commands() {
    let (command_tx, mut command_rx) = mpsc::unbounded_channel();
    let (host_key_decision_tx, mut host_key_decision_rx) = mpsc::unbounded_channel();
    let handle = ConnectionHandle {
        command_tx,
        host_key_decision_tx,
        host_key_verification_pending: Arc::new(AtomicBool::new(true)),
    };
    let size = PtySize::new(120, 40);

    handle
        .send_input(b"pwd\n".to_vec())
        .expect("input should be sent");
    handle.resize(size).expect("resize should be sent");
    handle
        .read_directory(7, "/home/test")
        .expect("directory request should be sent");
    handle
        .read_directory_tree(8, "/home/test/projects")
        .expect("directory tree request should be sent");
    handle
        .read_file(9, "/home/test/notes.txt")
        .expect("file request should be sent");
    handle
        .write_file(10, "/home/test/notes.txt", b"old".to_vec(), b"new".to_vec())
        .expect("file write should be sent");
    handle
        .create_file(11, "/home/test/new.txt")
        .expect("file creation should be sent");
    handle
        .create_directories(
            12,
            vec!["/home/test/new".into(), "/home/test/new/nested".into()],
        )
        .expect("directory creation should be sent");
    handle
        .delete_paths(13, vec!["/home/test/old".into()])
        .expect("recursive deletion should be sent");
    handle
        .upload_file(
            14,
            PathBuf::from("/tmp/upload.txt"),
            "/home/test/upload.txt",
            false,
        )
        .expect("upload should be sent");
    handle
        .download_file(
            15,
            "/home/test/download.txt",
            PathBuf::from("/tmp/download.txt"),
            true,
        )
        .expect("download should be sent");
    handle
        .cancel_transfer(15)
        .expect("transfer cancellation should be sent");
    handle
        .set_performance_monitoring(true)
        .expect("performance monitoring should be sent");
    handle
        .trust_host_key()
        .expect("host key trust should be sent");
    handle
        .host_key_verification_pending
        .store(true, Ordering::Release);
    handle
        .reject_host_key()
        .expect("host key rejection should be sent");
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
        command_rx.try_recv().expect("directory command"),
        ConnectionCommand::ReadDirectory {
            request_id: 7,
            path: "/home/test".into(),
        }
    );
    assert_eq!(
        command_rx.try_recv().expect("directory tree command"),
        ConnectionCommand::ReadDirectoryTree {
            request_id: 8,
            path: "/home/test/projects".into(),
        }
    );
    assert_eq!(
        command_rx.try_recv().expect("file command"),
        ConnectionCommand::ReadFile {
            request_id: 9,
            path: "/home/test/notes.txt".into(),
        }
    );
    assert_eq!(
        command_rx.try_recv().expect("file write command"),
        ConnectionCommand::WriteFile {
            request_id: 10,
            path: "/home/test/notes.txt".into(),
            expected_contents: b"old".to_vec(),
            contents: b"new".to_vec(),
        }
    );
    assert_eq!(
        command_rx.try_recv().expect("file creation command"),
        ConnectionCommand::CreateFile {
            request_id: 11,
            path: "/home/test/new.txt".into(),
        }
    );
    assert_eq!(
        command_rx.try_recv().expect("directory creation command"),
        ConnectionCommand::CreateDirectories {
            request_id: 12,
            paths: vec!["/home/test/new".into(), "/home/test/new/nested".into()],
        }
    );
    assert_eq!(
        command_rx.try_recv().expect("recursive deletion command"),
        ConnectionCommand::DeletePaths {
            request_id: 13,
            paths: vec!["/home/test/old".into()],
        }
    );
    assert_eq!(
        command_rx.try_recv().expect("upload command"),
        ConnectionCommand::UploadFile {
            transfer_id: 14,
            local_path: PathBuf::from("/tmp/upload.txt"),
            remote_path: "/home/test/upload.txt".into(),
            overwrite: false,
        }
    );
    assert_eq!(
        command_rx.try_recv().expect("download command"),
        ConnectionCommand::DownloadFile {
            transfer_id: 15,
            remote_path: "/home/test/download.txt".into(),
            local_path: PathBuf::from("/tmp/download.txt"),
            overwrite: true,
        }
    );
    assert_eq!(
        command_rx
            .try_recv()
            .expect("transfer cancellation command"),
        ConnectionCommand::CancelTransfer { transfer_id: 15 }
    );
    assert_eq!(
        command_rx.try_recv().expect("performance command"),
        ConnectionCommand::SetPerformanceMonitoring(true)
    );
    assert_eq!(
        host_key_decision_rx.try_recv().expect("trust decision"),
        HostKeyDecision::Trust
    );
    assert_eq!(
        host_key_decision_rx.try_recv().expect("reject decision"),
        HostKeyDecision::Reject
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
    let (host_key_decision_tx, _host_key_decision_rx) = mpsc::unbounded_channel();
    let handle = ConnectionHandle {
        command_tx,
        host_key_decision_tx,
        host_key_verification_pending: Arc::new(AtomicBool::new(false)),
    };

    drop(command_rx);

    let error = handle
        .disconnect()
        .expect_err("closed worker should reject commands");

    assert_eq!(error.kind(), SshErrorKind::InvalidState);
}

#[test]
fn host_key_cannot_be_trusted_before_verification_is_requested() {
    let (command_tx, _command_rx) = mpsc::unbounded_channel();
    let (host_key_decision_tx, mut host_key_decision_rx) = mpsc::unbounded_channel();
    let handle = ConnectionHandle {
        command_tx,
        host_key_decision_tx,
        host_key_verification_pending: Arc::new(AtomicBool::new(false)),
    };

    let error = handle
        .trust_host_key()
        .expect_err("preemptive trust must be rejected");

    assert_eq!(error.kind(), SshErrorKind::InvalidState);
    assert!(host_key_decision_rx.try_recv().is_err());
}

#[tokio::test]
async fn resize_is_retained_while_waiting_for_host_key_decision() {
    let (command_tx, mut command_rx) = mpsc::unbounded_channel();
    let (decision_tx, mut decision_rx) = mpsc::unbounded_channel();
    let mut latest_size = PtySize::default();
    let resized = PtySize::new(132, 43);

    command_tx
        .send(ConnectionCommand::Resize(resized))
        .expect("resize should be queued");

    let send_decision = async move {
        tokio::task::yield_now().await;
        decision_tx
            .send(HostKeyDecision::Trust)
            .expect("decision should be queued");
    };
    let (result, ()) = tokio::join!(
        wait_for_host_key_decision(&mut decision_rx, &mut command_rx, &mut latest_size),
        send_decision,
    );

    assert!(matches!(
        result,
        PendingResult::Completed(Ok(HostKeyDecision::Trust))
    ));
    assert_eq!(latest_size, resized);
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
        receiver.try_next_event(),
        Some(ConnectionEvent::StateChanged(SessionState::Authenticating))
    );
    assert_eq!(
        receiver.try_next_event(),
        Some(ConnectionEvent::Resized(resized))
    );
    assert_eq!(
        receiver.try_next_event(),
        Some(ConnectionEvent::Shell(ShellEvent::Output(
            b"prompt".to_vec()
        )))
    );
    assert_eq!(receiver.try_next_event(), None);
}

fn test_profile(port: u16) -> ConnectionProfile {
    ConnectionProfile::new("worker-test", "Worker Test", "127.0.0.1", port, "tester")
}

async fn next_event(receiver: &mut ConnectionEventReceiver) -> ConnectionEvent {
    tokio::time::timeout(Duration::from_secs(5), receiver.next_event())
        .await
        .expect("worker event should not time out")
        .expect("worker should still be running")
}

#[tokio::test]
async fn worker_reports_connection_failure() {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("temporary TCP listener");
    let port = listener.local_addr().expect("local address").port();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("worker TCP connection");
        drop(stream);
    });

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
    assert_eq!(
        next_event(&mut events).await,
        ConnectionEvent::ConnectionStageChanged(ConnectionStage::Target {
            profile_id: "worker-test".into(),
        })
    );

    let ConnectionEvent::Failed(error) = next_event(&mut events).await else {
        panic!("connection refusal should produce a failure event");
    };

    server.await.expect("test server should stop");
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
    assert_eq!(
        next_event(&mut events).await,
        ConnectionEvent::ConnectionStageChanged(ConnectionStage::Target {
            profile_id: "worker-test".into(),
        })
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
