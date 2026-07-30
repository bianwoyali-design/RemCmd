use std::{
    fmt,
    io::{Read, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, RecvTimeoutError},
    },
    thread,
    time::Duration,
};

use portable_pty::{CommandBuilder, PtySize as PortablePtySize, native_pty_system};
use tokio::sync::mpsc as tokio_mpsc;

const COMMAND_POLL_INTERVAL: Duration = Duration::from_millis(16);
const OUTPUT_BUFFER_SIZE: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalPtySize {
    pub columns: u32,
    pub rows: u32,
    pub pixel_width: u32,
    pub pixel_height: u32,
}

impl LocalPtySize {
    pub const fn new(columns: u32, rows: u32) -> Self {
        Self {
            columns,
            rows,
            pixel_width: 0,
            pixel_height: 0,
        }
    }

    pub const fn with_pixels(mut self, pixel_width: u32, pixel_height: u32) -> Self {
        self.pixel_width = pixel_width;
        self.pixel_height = pixel_height;
        self
    }

    fn portable(self) -> PortablePtySize {
        PortablePtySize {
            rows: clamp_u16(self.rows),
            cols: clamp_u16(self.columns),
            pixel_width: clamp_u16(self.pixel_width),
            pixel_height: clamp_u16(self.pixel_height),
        }
    }
}

impl Default for LocalPtySize {
    fn default() -> Self {
        Self::new(80, 24)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalTerminalError {
    message: String,
}

impl LocalTerminalError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for LocalTerminalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for LocalTerminalError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LocalTerminalEvent {
    Started,
    Output(Vec<u8>),
    Resized(LocalPtySize),
    Exited {
        exit_code: u32,
        signal: Option<String>,
    },
    Failed(LocalTerminalError),
}

enum LocalTerminalCommand {
    Input(Vec<u8>),
    Resize(LocalPtySize),
    Disconnect,
}

#[derive(Clone)]
pub struct LocalTerminalHandle {
    commands: mpsc::Sender<LocalTerminalCommand>,
}

impl LocalTerminalHandle {
    pub fn send_input(&self, data: Vec<u8>) -> Result<(), LocalTerminalError> {
        self.send(LocalTerminalCommand::Input(data))
    }

    pub fn resize(&self, size: LocalPtySize) -> Result<(), LocalTerminalError> {
        self.send(LocalTerminalCommand::Resize(size))
    }

    pub fn disconnect(&self) -> Result<(), LocalTerminalError> {
        self.send(LocalTerminalCommand::Disconnect)
    }

    fn send(&self, command: LocalTerminalCommand) -> Result<(), LocalTerminalError> {
        self.commands
            .send(command)
            .map_err(|_| LocalTerminalError::new("local terminal worker is no longer running"))
    }
}

pub struct LocalTerminalEventReceiver {
    events: tokio_mpsc::UnboundedReceiver<LocalTerminalEvent>,
}

impl LocalTerminalEventReceiver {
    pub async fn next_event(&mut self) -> Option<LocalTerminalEvent> {
        self.events.recv().await
    }

    pub fn try_next_event(&mut self) -> Option<LocalTerminalEvent> {
        self.events.try_recv().ok()
    }
}

pub struct LocalTerminal {
    handle: LocalTerminalHandle,
    events: LocalTerminalEventReceiver,
}

impl LocalTerminal {
    pub fn spawn(size: LocalPtySize) -> Self {
        Self::spawn_command(CommandBuilder::new_default_prog(), size)
    }

    pub fn split(self) -> (LocalTerminalHandle, LocalTerminalEventReceiver) {
        (self.handle, self.events)
    }

    fn spawn_command(command: CommandBuilder, size: LocalPtySize) -> Self {
        let (command_sender, command_receiver) = mpsc::channel();
        let (event_sender, event_receiver) = tokio_mpsc::unbounded_channel();
        let failed_sender = event_sender.clone();

        if let Err(error) = thread::Builder::new()
            .name("remcmd-local-terminal".into())
            .spawn(move || run_local_terminal(command, size, command_receiver, event_sender))
        {
            let _ = failed_sender.send(LocalTerminalEvent::Failed(LocalTerminalError::new(
                format!("failed to start local terminal worker: {error}"),
            )));
        }

        Self {
            handle: LocalTerminalHandle {
                commands: command_sender,
            },
            events: LocalTerminalEventReceiver {
                events: event_receiver,
            },
        }
    }
}

fn run_local_terminal(
    mut command: CommandBuilder,
    initial_size: LocalPtySize,
    commands: mpsc::Receiver<LocalTerminalCommand>,
    events: tokio_mpsc::UnboundedSender<LocalTerminalEvent>,
) {
    command.env("TERM", "xterm-256color");
    command.env("COLORTERM", "truecolor");
    if let Some(home) = home_directory() {
        command.cwd(home);
    }

    let pty_system = native_pty_system();
    let pair = match pty_system.openpty(initial_size.portable()) {
        Ok(pair) => pair,
        Err(error) => {
            send_failure(&events, format!("failed to open local PTY: {error}"));
            return;
        }
    };
    let mut child = match pair.slave.spawn_command(command) {
        Ok(child) => child,
        Err(error) => {
            send_failure(&events, format!("failed to start the local shell: {error}"));
            return;
        }
    };
    drop(pair.slave);

    let mut reader = match pair.master.try_clone_reader() {
        Ok(reader) => reader,
        Err(error) => {
            let _ = child.kill();
            send_failure(&events, format!("failed to open local PTY output: {error}"));
            return;
        }
    };
    let mut writer = match pair.master.take_writer() {
        Ok(writer) => writer,
        Err(error) => {
            let _ = child.kill();
            send_failure(&events, format!("failed to open local PTY input: {error}"));
            return;
        }
    };

    let reader_failed = Arc::new(AtomicBool::new(false));
    let reader_failed_worker = Arc::clone(&reader_failed);
    let reader_events = events.clone();
    let _ = thread::Builder::new()
        .name("remcmd-local-terminal-output".into())
        .spawn(move || {
            let mut buffer = vec![0; OUTPUT_BUFFER_SIZE];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(read) => {
                        if reader_events
                            .send(LocalTerminalEvent::Output(buffer[..read].to_vec()))
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(error) => {
                        reader_failed_worker.store(true, Ordering::Release);
                        send_failure(
                            &reader_events,
                            format!("failed to read local PTY output: {error}"),
                        );
                        break;
                    }
                }
            }
        });

    if events.send(LocalTerminalEvent::Started).is_err() {
        let _ = child.kill();
        return;
    }

    loop {
        if reader_failed.load(Ordering::Acquire) {
            let _ = child.kill();
            return;
        }

        match child.try_wait() {
            Ok(Some(status)) => {
                let _ = events.send(LocalTerminalEvent::Exited {
                    exit_code: status.exit_code(),
                    signal: status.signal().map(ToOwned::to_owned),
                });
                return;
            }
            Ok(None) => {}
            Err(error) => {
                send_failure(
                    &events,
                    format!("failed to inspect the local shell: {error}"),
                );
                let _ = child.kill();
                return;
            }
        }

        match commands.recv_timeout(COMMAND_POLL_INTERVAL) {
            Ok(LocalTerminalCommand::Input(data)) => {
                if let Err(error) = writer.write_all(&data).and_then(|_| writer.flush()) {
                    send_failure(&events, format!("failed to write local PTY input: {error}"));
                    let _ = child.kill();
                    return;
                }
            }
            Ok(LocalTerminalCommand::Resize(size)) => {
                if let Err(error) = pair.master.resize(size.portable()) {
                    send_failure(&events, format!("failed to resize local PTY: {error}"));
                    let _ = child.kill();
                    return;
                }
                if events.send(LocalTerminalEvent::Resized(size)).is_err() {
                    let _ = child.kill();
                    return;
                }
            }
            Ok(LocalTerminalCommand::Disconnect) | Err(RecvTimeoutError::Disconnected) => {
                if let Err(error) = child.kill() {
                    send_failure(&events, format!("failed to stop the local shell: {error}"));
                    return;
                }
                match child.wait() {
                    Ok(status) => {
                        let _ = events.send(LocalTerminalEvent::Exited {
                            exit_code: status.exit_code(),
                            signal: status.signal().map(ToOwned::to_owned),
                        });
                    }
                    Err(error) => {
                        send_failure(
                            &events,
                            format!("failed to wait for the local shell: {error}"),
                        );
                    }
                }
                return;
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
    }
}

fn send_failure(
    events: &tokio_mpsc::UnboundedSender<LocalTerminalEvent>,
    message: impl Into<String>,
) {
    let _ = events.send(LocalTerminalEvent::Failed(LocalTerminalError::new(message)));
}

fn home_directory() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(Into::into)
}

const fn clamp_u16(value: u32) -> u16 {
    if value > u16::MAX as u32 {
        u16::MAX
    } else {
        value as u16
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pty_size_clamps_platform_dimensions() {
        let size = LocalPtySize {
            columns: u32::MAX,
            rows: 24,
            pixel_width: u32::MAX,
            pixel_height: 0,
        }
        .portable();

        assert_eq!(size.cols, u16::MAX);
        assert_eq!(size.rows, 24);
        assert_eq!(size.pixel_width, u16::MAX);
        assert_eq!(size.pixel_height, 0);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn local_terminal_reports_output_and_exit() {
        let mut command = CommandBuilder::new("/bin/sh");
        command.args(["-c", "printf remcmd-local"]);
        let (handle, mut events) =
            LocalTerminal::spawn_command(command, LocalPtySize::default()).split();
        let mut output = Vec::new();
        let mut exited = false;

        while let Some(event) = events.next_event().await {
            match event {
                LocalTerminalEvent::Output(data) => output.extend(data),
                LocalTerminalEvent::Exited { exit_code, .. } => {
                    assert_eq!(exit_code, 0);
                    exited = true;
                    break;
                }
                LocalTerminalEvent::Failed(error) => panic!("{error}"),
                LocalTerminalEvent::Started | LocalTerminalEvent::Resized(_) => {}
            }
        }

        drop(handle);
        assert!(exited);
        assert!(String::from_utf8_lossy(&output).contains("remcmd-local"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn local_terminal_accepts_input_and_resize() {
        let command = CommandBuilder::new("/bin/sh");
        let (handle, mut events) =
            LocalTerminal::spawn_command(command, LocalPtySize::default()).split();
        let resized = LocalPtySize::new(100, 40).with_pixels(800, 640);
        let mut output = Vec::new();
        let mut resize_confirmed = false;

        handle.resize(resized).unwrap();
        handle
            .send_input(b"printf remcmd-input; exit\r".to_vec())
            .unwrap();

        tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(event) = events.next_event().await {
                match event {
                    LocalTerminalEvent::Output(data) => output.extend(data),
                    LocalTerminalEvent::Resized(size) => resize_confirmed = size == resized,
                    LocalTerminalEvent::Exited { .. } => break,
                    LocalTerminalEvent::Failed(error) => panic!("{error}"),
                    LocalTerminalEvent::Started => {}
                }
            }
        })
        .await
        .expect("local terminal should exit");

        drop(handle);
        assert!(resize_confirmed);
        assert!(String::from_utf8_lossy(&output).contains("remcmd-input"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn local_terminal_disconnect_stops_the_child() {
        let command = CommandBuilder::new("/bin/sh");
        let (handle, mut events) =
            LocalTerminal::spawn_command(command, LocalPtySize::default()).split();

        handle.disconnect().unwrap();
        let exit_code = tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(event) = events.next_event().await {
                match event {
                    LocalTerminalEvent::Exited { exit_code, .. } => return exit_code,
                    LocalTerminalEvent::Failed(error) => panic!("{error}"),
                    LocalTerminalEvent::Started
                    | LocalTerminalEvent::Output(_)
                    | LocalTerminalEvent::Resized(_) => {}
                }
            }
            panic!("local terminal event stream ended before exit");
        })
        .await
        .expect("local terminal should stop after disconnect");

        assert_ne!(exit_code, 0);
    }
}
