/// Dimensions reported to a local or remote pseudo-terminal.
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
