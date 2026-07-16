use std::error::Error;
use std::fmt::{self, Display, Formatter};

pub const MIN_COLUMNS: usize = 2;
pub const MIN_ROWS: usize = 1;
pub const PALETTE_SIZE: usize = 269;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TerminalSize {
    columns: usize,
    rows: usize,
}

impl TerminalSize {
    pub fn new(columns: usize, rows: usize) -> Result<Self, InvalidTerminalSize> {
        if columns < MIN_COLUMNS || rows < MIN_ROWS {
            return Err(InvalidTerminalSize { columns, rows });
        }

        Ok(Self { columns, rows })
    }

    pub const fn columns(self) -> usize {
        self.columns
    }

    pub const fn rows(self) -> usize {
        self.rows
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidTerminalSize {
    pub columns: usize,
    pub rows: usize,
}

impl Display for InvalidTerminalSize {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "terminal size must be at least {MIN_COLUMNS} columns by {MIN_ROWS} row, got {} by {}",
            self.columns, self.rows
        )
    }
}

impl Error for InvalidTerminalSize {}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Osc52Mode {
    Disabled,
    #[default]
    CopyOnly,
    PasteOnly,
    CopyPaste,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalConfig {
    pub scrollback_lines: usize,
    pub kitty_keyboard: bool,
    pub osc52: Osc52Mode,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            scrollback_lines: 10_000,
            kitty_keyboard: false,
            osc52: Osc52Mode::CopyOnly,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Scroll {
    Lines(i32),
    PageUp,
    PageDown,
    Top,
    Bottom,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Rgb {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

impl Rgb {
    pub const fn new(red: u8, green: u8, blue: u8) -> Self {
        Self { red, green, blue }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(usize)]
pub enum NamedColor {
    Black = 0,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    White,
    BrightBlack,
    BrightRed,
    BrightGreen,
    BrightYellow,
    BrightBlue,
    BrightMagenta,
    BrightCyan,
    BrightWhite,
    Foreground = 256,
    Background,
    Cursor,
    DimBlack,
    DimRed,
    DimGreen,
    DimYellow,
    DimBlue,
    DimMagenta,
    DimCyan,
    DimWhite,
    BrightForeground,
    DimForeground,
}

impl NamedColor {
    pub const fn palette_index(self) -> usize {
        self as usize
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerminalColor {
    Named(NamedColor),
    Indexed(u8),
    Rgb(Rgb),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum UnderlineStyle {
    #[default]
    None,
    Single,
    Double,
    Curly,
    Dotted,
    Dashed,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CellAttributes(u16);

impl CellAttributes {
    pub const NONE: Self = Self(0);
    pub const BOLD: Self = Self(1 << 0);
    pub const ITALIC: Self = Self(1 << 1);
    pub const DIM: Self = Self(1 << 2);
    pub const INVERSE: Self = Self(1 << 3);
    pub const HIDDEN: Self = Self(1 << 4);
    pub const STRIKEOUT: Self = Self(1 << 5);
    pub const WRAPPED: Self = Self(1 << 6);
    pub const WIDE: Self = Self(1 << 7);
    pub const WIDE_SPACER: Self = Self(1 << 8);
    pub const LEADING_WIDE_SPACER: Self = Self(1 << 9);

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub(crate) fn insert(&mut self, other: Self) {
        self.0 |= other.0;
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Hyperlink {
    pub id: String,
    pub uri: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalCell {
    pub row: usize,
    pub column: usize,
    pub character: char,
    pub combining_characters: Vec<char>,
    pub foreground: TerminalColor,
    pub background: TerminalColor,
    pub underline_color: Option<TerminalColor>,
    pub underline: UnderlineStyle,
    pub attributes: CellAttributes,
    pub hyperlink: Option<Hyperlink>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CursorShape {
    Block,
    Underline,
    Beam,
    HollowBlock,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TerminalCursor {
    pub row: usize,
    pub column: usize,
    pub shape: CursorShape,
    pub blinking: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TerminalModes(u32);

impl TerminalModes {
    pub const NONE: Self = Self(0);
    pub const SHOW_CURSOR: Self = Self(1 << 0);
    pub const APPLICATION_CURSOR: Self = Self(1 << 1);
    pub const APPLICATION_KEYPAD: Self = Self(1 << 2);
    pub const MOUSE_REPORT_CLICK: Self = Self(1 << 3);
    pub const BRACKETED_PASTE: Self = Self(1 << 4);
    pub const SGR_MOUSE: Self = Self(1 << 5);
    pub const MOUSE_MOTION: Self = Self(1 << 6);
    pub const LINE_WRAP: Self = Self(1 << 7);
    pub const LINE_FEED_NEW_LINE: Self = Self(1 << 8);
    pub const ORIGIN: Self = Self(1 << 9);
    pub const INSERT: Self = Self(1 << 10);
    pub const FOCUS_REPORTING: Self = Self(1 << 11);
    pub const ALTERNATE_SCREEN: Self = Self(1 << 12);
    pub const MOUSE_DRAG: Self = Self(1 << 13);
    pub const UTF8_MOUSE: Self = Self(1 << 14);
    pub const ALTERNATE_SCROLL: Self = Self(1 << 15);
    pub const VI: Self = Self(1 << 16);
    pub const DISAMBIGUATE_ESCAPE_CODES: Self = Self(1 << 17);
    pub const REPORT_EVENT_TYPES: Self = Self(1 << 18);
    pub const REPORT_ALTERNATE_KEYS: Self = Self(1 << 19);
    pub const REPORT_ALL_KEYS_AS_ESCAPE_CODES: Self = Self(1 << 20);
    pub const REPORT_ASSOCIATED_TEXT: Self = Self(1 << 21);

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub(crate) fn insert(&mut self, other: Self) {
        self.0 |= other.0;
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PaletteOverrides {
    colors: [Option<Rgb>; PALETTE_SIZE],
}

impl Default for PaletteOverrides {
    fn default() -> Self {
        Self {
            colors: [None; PALETTE_SIZE],
        }
    }
}

impl PaletteOverrides {
    pub fn get(&self, index: usize) -> Option<Rgb> {
        self.colors.get(index).copied().flatten()
    }

    pub(crate) fn set(&mut self, index: usize, color: Option<Rgb>) {
        if let Some(slot) = self.colors.get_mut(index) {
            *slot = color;
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalSnapshot {
    pub size: TerminalSize,
    pub cells: Vec<TerminalCell>,
    pub cursor: Option<TerminalCursor>,
    pub display_offset: usize,
    pub history_size: usize,
    pub modes: TerminalModes,
    pub palette_overrides: PaletteOverrides,
}

impl TerminalSnapshot {
    pub fn cell(&self, row: usize, column: usize) -> Option<&TerminalCell> {
        if row >= self.size.rows() || column >= self.size.columns() {
            return None;
        }

        self.cells.get(row * self.size.columns() + column)
    }

    pub fn row_text(&self, row: usize) -> Option<String> {
        if row >= self.size.rows() {
            return None;
        }

        let start = row * self.size.columns();
        let end = start + self.size.columns();
        let mut text = String::with_capacity(self.size.columns());

        for cell in &self.cells[start..end] {
            if cell.attributes.contains(CellAttributes::WIDE_SPACER)
                || cell
                    .attributes
                    .contains(CellAttributes::LEADING_WIDE_SPACER)
            {
                continue;
            }

            text.push(cell.character);
            text.extend(cell.combining_characters.iter());
        }

        Some(text)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DamageRange {
    pub row: usize,
    pub left: usize,
    pub right: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TerminalDamage {
    Full,
    Partial(Vec<DamageRange>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Clipboard {
    Clipboard,
    Selection,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TextAreaSize {
    pub rows: u16,
    pub columns: u16,
    pub cell_width: u16,
    pub cell_height: u16,
}
