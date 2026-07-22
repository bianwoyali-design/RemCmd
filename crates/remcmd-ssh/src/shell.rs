use std::collections::VecDeque;

use russh::{Channel, ChannelMsg, ChannelReadHalf, ChannelWriteHalf, Pty, client};

use crate::{SshError, SshErrorKind, shell_integration::ShellIntegration};

const DEFAULT_TERMINAL_TYPE: &str = "xterm-256color";
const INTEGRATION_READY_MARKER: &[u8] = b"\x1b]777;remcmd-shell-ready\x07";
const INTEGRATION_COMMAND_PREFIX: &str = " stty -echo; ";
const INTEGRATION_READY_COMMAND: &str = "stty echo; printf '\\033]777;remcmd-shell-ready\\007'; __remcmd_last_cwd=$PWD; printf '\\r\\033[2K\\033]7;file://%s\\007' \"$PWD\"\r";

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
    initial_events: VecDeque<ShellEvent>,
}

impl SshShell {
    pub(crate) async fn open<H>(
        handle: &client::Handle<H>,
        size: PtySize,
        shell_integration: Option<&ShellIntegration>,
    ) -> Result<Self, SshError>
    where
        H: client::Handler,
    {
        let mut channel = handle
            .channel_open_session()
            .await
            .map_err(SshError::from)?;

        let terminal_modes = if shell_integration.is_some() {
            vec![(Pty::ECHO, 0)]
        } else {
            Vec::new()
        };

        channel
            .request_pty(
                true,
                DEFAULT_TERMINAL_TYPE,
                size.columns,
                size.rows,
                size.pixel_width,
                size.pixel_height,
                &terminal_modes,
            )
            .await
            .map_err(SshError::from)?;

        Self::wait_for_request_success(&mut channel, "PTY").await?;

        channel.request_shell(true).await.map_err(SshError::from)?;

        Self::wait_for_request_success(&mut channel, "shell").await?;

        let initial_events = if let Some(shell_integration) = shell_integration {
            Self::install_shell_integration(&mut channel, shell_integration).await?
        } else {
            VecDeque::new()
        };

        Ok(Self {
            channel,
            initial_events,
        })
    }

    async fn install_shell_integration(
        channel: &mut Channel<client::Msg>,
        shell_integration: &ShellIntegration,
    ) -> Result<VecDeque<ShellEvent>, SshError> {
        let install_command = format!(
            "{INTEGRATION_COMMAND_PREFIX}{}; {INTEGRATION_READY_COMMAND}",
            shell_integration.install_command()
        );
        channel
            .data_bytes(install_command.into_bytes())
            .await
            .map_err(SshError::from)?;

        let (_, ready_output) = Self::read_until_marker(channel, INTEGRATION_READY_MARKER).await?;
        let mut initial_events = VecDeque::new();

        // The first prompt can span multiple lines, and there is no protocol
        // boundary that reliably separates it from MOTD output. Drop all
        // pre-marker output so installing the hook cannot duplicate a prompt.
        if !ready_output.is_empty() {
            initial_events.push_back(ShellEvent::Output(ready_output));
        }

        Ok(initial_events)
    }

    async fn read_until_marker(
        channel: &mut Channel<client::Msg>,
        marker: &[u8],
    ) -> Result<(Vec<u8>, Vec<u8>), SshError> {
        let mut output = Vec::new();

        loop {
            match channel.wait().await {
                Some(ChannelMsg::Data { data }) => {
                    output.extend_from_slice(&data);
                    if let Some(index) = find_bytes(&output, marker) {
                        let after_marker = output.split_off(index + marker.len());
                        output.truncate(index);
                        return Ok((output, after_marker));
                    }
                }

                Some(ChannelMsg::ExtendedData { data, .. }) => {
                    output.extend_from_slice(&data);
                }

                Some(ChannelMsg::Failure) => {
                    return Err(SshError::new(
                        SshErrorKind::Protocol,
                        "server rejected shell integration input",
                    ));
                }

                Some(ChannelMsg::ExitStatus { exit_status }) => {
                    return Err(SshError::new(
                        SshErrorKind::Protocol,
                        format!("remote shell exited with status {exit_status} during startup"),
                    ));
                }

                Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) | None => {
                    return Err(SshError::new(
                        SshErrorKind::Network,
                        "channel closed while installing shell integration",
                    ));
                }

                Some(_) => {}
            }
        }
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
                pending_events: self.initial_events,
            },
            SshShellWriter { write_half },
        )
    }
}

pub struct SshShellReader {
    read_half: ChannelReadHalf,
    pending_events: VecDeque<ShellEvent>,
}

pub struct SshShellWriter {
    write_half: ChannelWriteHalf<client::Msg>,
}

impl SshShellReader {
    pub async fn next_event(&mut self) -> ShellEvent {
        if let Some(event) = self.pending_events.pop_front() {
            return event;
        }

        loop {
            match self.read_half.wait().await {
                Some(ChannelMsg::Data { data }) => {
                    return ShellEvent::Output(data.to_vec());
                }

                Some(ChannelMsg::ExtendedData { data, ext }) => {
                    return ShellEvent::ExtendedOutput {
                        code: ext,
                        data: data.to_vec(),
                    };
                }

                Some(ChannelMsg::ExitStatus { exit_status }) => {
                    return ShellEvent::ExitStatus(exit_status);
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

                    return ShellEvent::ExitSignal {
                        signal,
                        core_dumped,
                        message: error_message,
                    };
                }

                Some(ChannelMsg::Eof) => return ShellEvent::Eof,
                Some(ChannelMsg::Close) | None => return ShellEvent::Closed,

                // Internal protocol messages are not terminal output.
                Some(_) => {}
            }
        }
    }
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

impl SshShellWriter {
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
