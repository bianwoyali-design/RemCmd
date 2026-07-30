use std::time::{Duration, Instant};

use russh::{Channel, ChannelMsg, ChannelReadHalf, ChannelWriteHalf, client};

use crate::{SshError, SshErrorKind, shell_integration};

const DEFAULT_TERMINAL_TYPE: &str = "xterm-256color";
const INTEGRATION_READY_MARKER: &[u8] = b"\x1b]777;remcmd-shell-ready\x07";
const INTEGRATION_READY_COMMAND: &str = "printf '\\033]777;remcmd-shell-ready\\007'; printf '\\r\\033[2K\\033]7;file://%s\\007' \"$PWD\"\r";
const INTEGRATION_STARTUP_TIMEOUT: Duration = Duration::from_millis(750);
const MAX_INTEGRATION_STARTUP_BYTES: usize = 64 * 1024;

/// Dimensions reported to the remote pseudo-terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PtySize {
    /// Terminal width measured in character cells.
    pub columns: u32,

    /// Terminal height measured in character cells.
    pub rows: u32,

    /// Optional rendered width in pixels. Zero means unspecified.
    pub pixel_width: u32,

    /// Optional rendered height in pixels. Zero means unspecified.
    pub pixel_height: u32,
}

impl PtySize {
    /// Creates a character-cell size without pixel dimensions.
    pub const fn new(columns: u32, rows: u32) -> Self {
        Self {
            columns,
            rows,
            pixel_width: 0,
            pixel_height: 0,
        }
    }

    /// Adds optional pixel dimensions reported by the UI.
    pub const fn with_pixels(mut self, pixel_width: u32, pixel_height: u32) -> Self {
        self.pixel_width = pixel_width;
        self.pixel_height = pixel_height;
        self
    }
}

impl Default for PtySize {
    fn default() -> Self {
        // Conventional terminal dimensions before the UI is measured.
        Self::new(80, 24)
    }
}

/// An observable event received from the remote shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellEvent {
    /// Standard terminal output received from the remote process.
    Output(Vec<u8>),

    /// Extended output, normally stderr when no PTY is active.
    ExtendedOutput { code: u32, data: Vec<u8> },

    /// Exit code reported by the remote process.
    ExitStatus(u32),

    /// The remote process was terminated by a signal.
    ExitSignal {
        signal: String,
        core_dumped: bool,
        message: String,
    },

    /// The remote side will not send more data.
    Eof,

    /// The SSH channel has closed.
    Closed,
}

pub struct SshShell {
    channel: Channel<client::Msg>,
    filter_integration_startup: bool,
}

impl SshShell {
    pub(crate) async fn open<H>(
        handle: &client::Handle<H>,
        size: PtySize,
        install_cwd_hook: bool,
    ) -> Result<Self, SshError>
    where
        H: client::Handler,
    {
        let mut channel = handle
            .channel_open_session()
            .await
            .map_err(SshError::from)?;

        channel
            .request_pty(
                true,
                DEFAULT_TERMINAL_TYPE,
                size.columns,
                size.rows,
                size.pixel_width,
                size.pixel_height,
                &[],
            )
            .await
            .map_err(SshError::from)?;

        channel.request_shell(true).await.map_err(SshError::from)?;

        if install_cwd_hook {
            channel
                .data_bytes(shell_integration::install_command(
                    INTEGRATION_READY_COMMAND,
                ))
                .await
                .map_err(SshError::from)?;
        }

        // Channel requests and data are processed in packet order. Queue shell
        // startup and its hidden integration input before waiting for replies so
        // high-latency connections do not pay a separate round trip per step.
        Self::wait_for_request_success(&mut channel, "PTY").await?;
        Self::wait_for_request_success(&mut channel, "shell").await?;

        Ok(Self {
            channel,
            filter_integration_startup: install_cwd_hook,
        })
    }

    async fn wait_for_request_success(
        channel: &mut Channel<client::Msg>,
        request: &str,
    ) -> Result<(), SshError> {
        loop {
            match channel.wait().await {
                Some(ChannelMsg::Success) => return Ok(()),

                Some(ChannelMsg::Failure) => {
                    return Err(SshError::new(
                        SshErrorKind::Protocol,
                        format!("server rejected {request} request"),
                    ));
                }

                Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) | None => {
                    return Err(SshError::new(
                        SshErrorKind::Network,
                        format!("channel closed while waiting for {request} request"),
                    ));
                }

                // No output should normally arrive before shell startup.
                // Ignore protocol messages unrelated to this request.
                Some(_) => {}
            }
        }
    }

    pub async fn close(&self) -> Result<(), SshError> {
        // Attempt close even when sending EOF fails.
        let eof_result = self.channel.eof().await;
        let close_result = self.channel.close().await;

        eof_result.map_err(SshError::from)?;
        close_result.map_err(SshError::from)
    }

    pub fn split(self) -> (SshShellReader, SshShellWriter) {
        let (read_half, write_half) = self.channel.split();

        (
            SshShellReader {
                read_half,
                startup_output: self.filter_integration_startup.then(Vec::new),
                startup_deadline: self
                    .filter_integration_startup
                    .then(|| Instant::now() + INTEGRATION_STARTUP_TIMEOUT),
                pending_event: None,
            },
            SshShellWriter { write_half },
        )
    }
}

pub struct SshShellReader {
    read_half: ChannelReadHalf,
    startup_output: Option<Vec<u8>>,
    startup_deadline: Option<Instant>,
    pending_event: Option<ShellEvent>,
}

pub struct SshShellWriter {
    write_half: ChannelWriteHalf<client::Msg>,
}

impl SshShellReader {
    pub(crate) fn begin_integration_filter(&mut self) {
        self.startup_output = Some(Vec::new());
        self.startup_deadline = Some(Instant::now() + INTEGRATION_STARTUP_TIMEOUT);
    }

    pub async fn next_event(&mut self) -> ShellEvent {
        if let Some(event) = self.pending_event.take() {
            return event;
        }

        loop {
            let message = if let Some(deadline) = self.startup_deadline {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    if let Some(output) = self.finish_startup_filter() {
                        return ShellEvent::Output(output);
                    }
                    continue;
                }

                match tokio::time::timeout(remaining, self.read_half.wait()).await {
                    Ok(message) => message,
                    Err(_) => {
                        if let Some(output) = self.finish_startup_filter() {
                            return ShellEvent::Output(output);
                        }
                        continue;
                    }
                }
            } else {
                self.read_half.wait().await
            };

            match message {
                Some(ChannelMsg::Data { data }) => {
                    if let Some(output) = self.startup_output.as_mut() {
                        output.extend_from_slice(&data);
                        let Some(index) = find_bytes(output, INTEGRATION_READY_MARKER) else {
                            if output.len() >= MAX_INTEGRATION_STARTUP_BYTES
                                && let Some(output) = self.finish_startup_filter()
                            {
                                return ShellEvent::Output(output);
                            }
                            continue;
                        };
                        let visible = output.split_off(index + INTEGRATION_READY_MARKER.len());
                        self.startup_output = None;
                        self.startup_deadline = None;
                        if visible.is_empty() {
                            continue;
                        }
                        return ShellEvent::Output(visible);
                    }
                    return ShellEvent::Output(data.to_vec());
                }

                Some(ChannelMsg::ExtendedData { data, ext }) => {
                    if let Some(output) = self.startup_output.as_mut() {
                        output.extend_from_slice(&data);
                        if output.len() >= MAX_INTEGRATION_STARTUP_BYTES
                            && let Some(output) = self.finish_startup_filter()
                        {
                            return ShellEvent::Output(output);
                        }
                        continue;
                    }
                    return ShellEvent::ExtendedOutput {
                        code: ext,
                        data: data.to_vec(),
                    };
                }

                Some(ChannelMsg::ExitStatus { exit_status }) => {
                    return self.finish_startup_before(ShellEvent::ExitStatus(exit_status));
                }

                Some(ChannelMsg::ExitSignal {
                    signal_name,
                    core_dumped,
                    error_message,
                    ..
                }) => {
                    // Standard signals use their Debug name, while custom
                    // signals retain the server-provided string.
                    let signal = match signal_name {
                        russh::Sig::Custom(name) => name,
                        signal => format!("{signal:?}"),
                    };

                    return self.finish_startup_before(ShellEvent::ExitSignal {
                        signal,
                        core_dumped,
                        message: error_message,
                    });
                }

                Some(ChannelMsg::Eof) => {
                    return self.finish_startup_before(ShellEvent::Eof);
                }
                Some(ChannelMsg::Close) | None => {
                    return self.finish_startup_before(ShellEvent::Closed);
                }

                // Internal protocol messages are not terminal output.
                Some(_) => {}
            }
        }
    }

    fn finish_startup_filter(&mut self) -> Option<Vec<u8>> {
        self.startup_deadline = None;
        let mut output = self.startup_output.take()?;
        let prompt_start = output
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |index| index + 1);
        let prompt = output.split_off(prompt_start);

        if prompt.is_empty() {
            // Clear a partially echoed integration command without exposing it.
            Some(b"\r\x1b[2K".to_vec())
        } else {
            Some(prompt)
        }
    }

    fn finish_startup_before(&mut self, event: ShellEvent) -> ShellEvent {
        if let Some(output) = self.finish_startup_filter() {
            self.pending_event = Some(event);
            ShellEvent::Output(output)
        } else {
            event
        }
    }
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

impl SshShellWriter {
    pub(crate) async fn install_cwd_hook(
        &self,
        shell: shell_integration::ShellKind,
    ) -> Result<(), SshError> {
        self.send_input(shell_integration::install_command_for_shell(
            shell,
            INTEGRATION_READY_COMMAND,
        ))
        .await
    }

    /// Sends raw keyboard or paste bytes to the remote terminal.
    pub async fn send_input(&self, data: impl Into<Vec<u8>>) -> Result<(), SshError> {
        let data: Vec<u8> = data.into();

        self.write_half
            .data_bytes(data)
            .await
            .map_err(SshError::from)
    }

    /// Reports a new terminal size to the remote PTY.
    pub async fn resize(&self, size: PtySize) -> Result<(), SshError> {
        self.write_half
            .window_change(size.columns, size.rows, size.pixel_width, size.pixel_height)
            .await
            .map_err(SshError::from)
    }

    /// Sends EOF and closes the writable channel half.
    pub async fn close(&self) -> Result<(), SshError> {
        let eof_result = self.write_half.eof().await;
        let close_result = self.write_half.close().await;

        eof_result.map_err(SshError::from)?;
        close_result.map_err(SshError::from)
    }
}

#[cfg(test)]
mod tests;
