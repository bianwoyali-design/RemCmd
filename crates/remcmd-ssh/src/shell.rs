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

#[cfg(test)]
mod tests {
    use super::*;

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
}
