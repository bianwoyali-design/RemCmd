use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};

use rand::rng;
use russh::{Channel, ChannelId, Pty, client, server};
use tokio::{net::TcpListener, task::JoinHandle};

use super::{PtySize, ShellEvent, SshShell};

/// Values observed by the temporary SSH server.
#[derive(Default)]
struct TestServerState {
    terminal_type: Option<String>,
    initial_size: Option<PtySize>,
    pty_modes: Vec<(Pty, u32)>,
    shell_requested: bool,
    resized_size: Option<PtySize>,
    input: Vec<u8>,
    integration_input: Vec<u8>,
}

/// Test server handler for one SSH connection.
struct TestServer {
    state: Arc<Mutex<TestServerState>>,
}

impl server::Handler for TestServer {
    type Error = russh::Error;

    /// Test authentication avoids passwords and personal key files.
    async fn auth_none(&mut self, _user: &str) -> Result<server::Auth, Self::Error> {
        Ok(server::Auth::Accept)
    }

    async fn channel_open_session(
        &mut self,
        _channel: Channel<server::Msg>,
        reply: server::ChannelOpenHandle,
        _session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        // Channel opening has a dedicated asynchronous reply handle.
        reply.accept().await;
        Ok(())
    }

    async fn pty_request(
        &mut self,
        channel: ChannelId,
        term: &str,
        col_width: u32,
        row_height: u32,
        pix_width: u32,
        pix_height: u32,
        modes: &[(Pty, u32)],
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        {
            // Release the mutex before sending protocol responses.
            let mut state = self.state.lock().expect("test state lock");

            state.terminal_type = Some(term.to_owned());
            state.initial_size = Some(PtySize {
                columns: col_width,
                rows: row_height,
                pixel_width: pix_width,
                pixel_height: pix_height,
            });
            state.pty_modes = modes.to_vec();
        }

        // SshShell requested want_reply=true, so a reply is required.
        session.channel_success(channel)?;
        Ok(())
    }

    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        let integrated = {
            let mut state = self.state.lock().expect("test state lock");
            state.shell_requested = true;
            state.pty_modes.contains(&(Pty::ECHO, 0))
        };
        session.channel_success(channel)?;

        // Initial output will prove that ShellEvent::Output works.
        let output = if integrated {
            b"welcome\r\ninitial-starship-prompt".to_vec()
        } else {
            b"ready\r\n".to_vec()
        };
        session.data(channel, output)?;
        Ok(())
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        if data
            .windows(b"remcmd-shell-start".len())
            .any(|window| window == b"remcmd-shell-start")
        {
            self.state
                .lock()
                .expect("test state lock")
                .integration_input
                .extend_from_slice(data);
            let (start, end) = super::INTEGRATION_START_MARKER.split_at(9);
            session.data(channel, start.to_vec())?;
            session.data(channel, end.to_vec())?;
        } else if data
            .windows(b"remcmd-shell-ready".len())
            .any(|window| window == b"remcmd-shell-ready")
        {
            self.state
                .lock()
                .expect("test state lock")
                .integration_input
                .extend_from_slice(data);
            let (start, end) = super::INTEGRATION_READY_MARKER.split_at(11);
            session.data(channel, start.to_vec())?;
            let mut final_output = end.to_vec();
            final_output
                .extend_from_slice(b"\r\x1b[2K\x1b]7;file:///home/tester\x07starship-prompt");
            session.data(channel, final_output)?;
        } else if data == b"exit\r" {
            self.state
                .lock()
                .expect("test state lock")
                .input
                .extend_from_slice(data);
            // Simulate a normal shell process exiting.
            session.exit_status_request(channel, 0)?;
            session.eof(channel)?;
            session.close(channel)?;
        } else {
            self.state
                .lock()
                .expect("test state lock")
                .input
                .extend_from_slice(data);
            // Echo ordinary input back as terminal output.
            session.data(channel, data.to_vec())?;
        }

        Ok(())
    }

    async fn window_change_request(
        &mut self,
        channel: ChannelId,
        col_width: u32,
        row_height: u32,
        pix_width: u32,
        pix_height: u32,
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        self.state.lock().expect("test state lock").resized_size = Some(PtySize {
            columns: col_width,
            rows: row_height,
            pixel_width: pix_width,
            pixel_height: pix_height,
        });

        // Send an acknowledgement as output so the test knows the
        // server processed the resize before inspecting shared state.
        session.channel_success(channel)?;
        session.data(channel, b"resized".to_vec())?;
        Ok(())
    }
}

/// Test client accepts the temporary server key generated below.
struct TestClient;

impl client::Handler for TestClient {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &russh::keys::PublicKey,
    ) -> Result<bool, Self::Error> {
        // Safe only because the server runs inside this test process.
        Ok(true)
    }
}

async fn start_test_server() -> (SocketAddr, JoinHandle<()>, Arc<Mutex<TestServerState>>) {
    let state = Arc::new(Mutex::new(TestServerState::default()));
    let handler_state = Arc::clone(&state);

    // Generate a temporary Host Key that exists only for this test.
    let host_key = russh::keys::PrivateKey::random(&mut rng(), russh::keys::Algorithm::Ed25519)
        .expect("temporary host key");

    let config = Arc::new(server::Config {
        auth_rejection_time: Duration::ZERO,
        inactivity_timeout: None,
        keys: vec![host_key],
        ..Default::default()
    });

    // Port zero asks the OS to allocate an available local port.
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("local test listener");

    let address = listener.local_addr().expect("local test address");

    let task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("test connection");

        if let Ok(running) = server::run_stream(
            config,
            stream,
            TestServer {
                state: handler_state,
            },
        )
        .await
        {
            // The session finishes when the test client disconnects.
            let _ = running.await;
        }
    });

    (address, task, state)
}

#[test]
fn default_pty_size_is_eighty_by_twenty_four() {
    let size = PtySize::default();

    assert_eq!(size.columns, 80);
    assert_eq!(size.rows, 24);
    assert_eq!(size.pixel_width, 0);
    assert_eq!(size.pixel_height, 0);
}

#[test]
fn pty_size_can_include_pixel_dimensions() {
    let size = PtySize::new(120, 40).with_pixels(1440, 900);

    assert_eq!(size.columns, 120);
    assert_eq!(size.rows, 40);
    assert_eq!(size.pixel_width, 1440);
    assert_eq!(size.pixel_height, 900);
}

#[tokio::test]
async fn local_test_server_accepts_connection_and_authentication() {
    let (address, server_task, _state) = start_test_server().await;

    let mut handle = client::connect(Arc::new(client::Config::default()), address, TestClient)
        .await
        .expect("test client connection");

    let authentication = handle
        .authenticate_none("tester")
        .await
        .expect("test authentication");

    assert!(authentication.success());

    handle
        .disconnect(russh::Disconnect::ByApplication, "test complete", "en")
        .await
        .expect("test disconnect");

    tokio::time::timeout(Duration::from_secs(1), server_task)
        .await
        .expect("test server should stop")
        .expect("test server task");
}

#[tokio::test]
async fn interactive_shell_supports_pty_io_resize_and_exit() {
    let (address, server_task, state) = start_test_server().await;

    let mut handle = client::connect(Arc::new(client::Config::default()), address, TestClient)
        .await
        .expect("test client connection");

    let authentication = handle
        .authenticate_none("tester")
        .await
        .expect("test authentication");

    assert!(authentication.success());

    let initial_size = PtySize::new(100, 30).with_pixels(1000, 600);

    let shell = SshShell::open(&handle, initial_size, None)
        .await
        .expect("interactive shell");

    let (mut reader, writer) = shell.split();

    assert_eq!(
        reader.next_event().await,
        ShellEvent::Output(b"ready\r\n".to_vec())
    );

    {
        let state = state.lock().expect("test state lock");

        assert_eq!(state.terminal_type.as_deref(), Some("xterm-256color"));
        assert_eq!(state.initial_size, Some(initial_size));
        assert!(state.pty_modes.is_empty());
        assert!(state.shell_requested);
    }

    writer
        .send_input(b"hello\r".to_vec())
        .await
        .expect("terminal input");

    assert_eq!(
        reader.next_event().await,
        ShellEvent::Output(b"hello\r".to_vec())
    );

    let resized = PtySize::new(132, 50).with_pixels(1320, 1000);

    writer.resize(resized).await.expect("terminal resize");

    assert_eq!(
        reader.next_event().await,
        ShellEvent::Output(b"resized".to_vec())
    );

    {
        let state = state.lock().expect("test state lock");

        assert_eq!(state.resized_size, Some(resized));
        assert_eq!(state.input, b"hello\r");
    }

    writer
        .send_input(b"exit\r".to_vec())
        .await
        .expect("exit command");

    assert_eq!(reader.next_event().await, ShellEvent::ExitStatus(0));
    assert_eq!(reader.next_event().await, ShellEvent::Eof);
    assert_eq!(reader.next_event().await, ShellEvent::Closed);

    handle
        .disconnect(russh::Disconnect::ByApplication, "test complete", "en")
        .await
        .expect("test disconnect");

    tokio::time::timeout(Duration::from_secs(1), server_task)
        .await
        .expect("test server should stop")
        .expect("test server task");
}

#[tokio::test]
async fn shell_integration_preserves_native_shell_and_stays_hidden() {
    let (address, server_task, state) = start_test_server().await;
    let mut handle = client::connect(Arc::new(client::Config::default()), address, TestClient)
        .await
        .expect("test client connection");
    assert!(
        handle
            .authenticate_none("tester")
            .await
            .expect("test authentication")
            .success()
    );

    let integration = crate::shell_integration::ShellIntegration::detect(b"/bin/bash").unwrap();
    let shell = SshShell::open(&handle, PtySize::default(), Some(&integration))
        .await
        .expect("integrated shell");
    let (mut reader, writer) = shell.split();

    assert_eq!(
        reader.next_event().await,
        ShellEvent::Output(b"welcome\r\n".to_vec())
    );
    assert_eq!(
        reader.next_event().await,
        ShellEvent::Output(b"\r\x1b[2K\x1b]7;file:///home/tester\x07starship-prompt".to_vec())
    );
    {
        let state = state.lock().expect("test state lock");
        assert!(state.shell_requested);
        assert_eq!(state.pty_modes, vec![(Pty::ECHO, 0)]);
        let integration_input = String::from_utf8_lossy(&state.integration_input);
        assert!(integration_input.contains("7;file://%s"));
        assert!(!integration_input.contains("PS1"));
        assert!(!integration_input.contains("PROMPT="));
    }

    writer
        .send_input(b"exit\r".to_vec())
        .await
        .expect("exit command");
    assert_eq!(reader.next_event().await, ShellEvent::ExitStatus(0));
    assert_eq!(reader.next_event().await, ShellEvent::Eof);
    assert_eq!(reader.next_event().await, ShellEvent::Closed);

    handle
        .disconnect(russh::Disconnect::ByApplication, "test complete", "en")
        .await
        .expect("test disconnect");
    tokio::time::timeout(Duration::from_secs(1), server_task)
        .await
        .expect("test server should stop")
        .expect("test server task");
}
