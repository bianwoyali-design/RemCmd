use alacritty_terminal::grid::{Dimensions, GridCell, Row, Scroll as AlacrittyScroll};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::{Cell as AlacrittyCell, Flags};
use alacritty_terminal::term::{
    Config as AlacrittyConfig, Osc52, Term, TermDamage as AlacrittyDamage, TermMode,
    point_to_viewport,
};
use alacritty_terminal::vte::ansi::{
    Color as AlacrittyColor, CursorShape as AlacrittyCursorShape,
    NamedColor as AlacrittyNamedColor, Processor, Rgb as AlacrittyRgb,
};

use crate::screen::{
    CellAttributes, CursorShape, DamageRange, Hyperlink, InvalidTerminalSize, NamedColor,
    Osc52Mode, PALETTE_SIZE, PaletteOverrides, Rgb, Scroll, TerminalCell, TerminalColor,
    TerminalConfig, TerminalCursor, TerminalDamage, TerminalModes, TerminalSize, TerminalSnapshot,
    UnderlineStyle,
};
use crate::{
    cwd::Osc7Parser,
    event::{EventQueue, TerminalEvent},
};

const MIN_RIGHT_ALIGNED_GAP: usize = 8;
const MAX_RIGHT_ALIGNED_SUFFIX: usize = 32;
const MAX_RIGHT_MARGIN: usize = 2;

pub struct TerminalEngine {
    parser: Processor,
    cwd_parser: Osc7Parser,
    terminal: Term<EventQueue>,
    events: EventQueue,
    size: TerminalSize,
}

impl TerminalEngine {
    pub fn new(columns: usize, rows: usize) -> Result<Self, InvalidTerminalSize> {
        Self::with_config(columns, rows, TerminalConfig::default())
    }

    pub fn with_config(
        columns: usize,
        rows: usize,
        config: TerminalConfig,
    ) -> Result<Self, InvalidTerminalSize> {
        let size = TerminalSize::new(columns, rows)?;
        let events = EventQueue::default();
        let terminal = Term::new(map_config(config), &size, events.clone());

        Ok(Self {
            parser: Processor::new(),
            cwd_parser: Osc7Parser::default(),
            terminal,
            events,
            size,
        })
    }

    pub fn process(&mut self, bytes: &[u8]) {
        let cwd_events = self.cwd_parser.advance(bytes);
        let mut start_offset = 0;
        for event in cwd_events {
            self.parser
                .advance(&mut self.terminal, &bytes[start_offset..event.end_offset]);
            self.events
                .push(TerminalEvent::WorkingDirectoryChanged(event.path));
            start_offset = event.end_offset;
        }
        self.parser
            .advance(&mut self.terminal, &bytes[start_offset..]);
    }

    pub fn resize(&mut self, columns: usize, rows: usize) -> Result<(), InvalidTerminalSize> {
        let size = TerminalSize::new(columns, rows)?;
        reanchor_right_aligned_rows(&mut self.terminal, columns);
        self.terminal.resize(size);
        self.size = size;
        Ok(())
    }

    pub fn set_config(&mut self, config: TerminalConfig) {
        self.terminal.set_options(map_config(config));
    }

    pub fn scroll(&mut self, scroll: Scroll) {
        let scroll = match scroll {
            Scroll::Lines(lines) => AlacrittyScroll::Delta(lines),
            Scroll::PageUp => AlacrittyScroll::PageUp,
            Scroll::PageDown => AlacrittyScroll::PageDown,
            Scroll::Top => AlacrittyScroll::Top,
            Scroll::Bottom => AlacrittyScroll::Bottom,
        };
        self.terminal.scroll_display(scroll);
    }

    pub const fn size(&self) -> TerminalSize {
        self.size
    }

    pub fn history_size(&self) -> usize {
        self.terminal.history_size()
    }

    pub fn display_offset(&self) -> usize {
        self.terminal.grid().display_offset()
    }

    pub fn modes(&self) -> TerminalModes {
        map_modes(*self.terminal.mode())
    }

    pub fn snapshot(&self) -> TerminalSnapshot {
        let content = self.terminal.renderable_content();
        let display_offset = content.display_offset;
        let cursor_style = self.terminal.cursor_style();
        let cursor = map_cursor(
            content.cursor.shape,
            content.cursor.point,
            cursor_style.blinking,
            display_offset,
            self.size,
        );

        let cells = content
            .display_iter
            .map(|indexed| {
                let row = (indexed.point.line.0 + display_offset as i32) as usize;
                map_cell(indexed.cell, row, indexed.point.column.0)
            })
            .collect();

        let mut palette_overrides = PaletteOverrides::default();
        debug_assert_eq!(alacritty_terminal::term::color::COUNT, PALETTE_SIZE);
        for index in 0..PALETTE_SIZE {
            palette_overrides.set(index, content.colors[index].map(map_rgb));
        }

        TerminalSnapshot {
            size: self.size,
            cells,
            cursor,
            display_offset,
            history_size: self.terminal.history_size(),
            modes: map_modes(content.mode),
            palette_overrides,
        }
    }

    pub fn take_damage(&mut self) -> TerminalDamage {
        let damage = match self.terminal.damage() {
            AlacrittyDamage::Full => TerminalDamage::Full,
            AlacrittyDamage::Partial(lines) => TerminalDamage::Partial(
                lines
                    .map(|line| DamageRange {
                        row: line.line,
                        left: line.left,
                        right: line.right,
                    })
                    .collect(),
            ),
        };
        self.terminal.reset_damage();
        damage
    }

    pub fn drain_events(&self) -> Vec<TerminalEvent> {
        self.events.drain()
    }
}

fn reanchor_right_aligned_rows(terminal: &mut Term<EventQueue>, target_columns: usize) {
    if terminal.mode().contains(TermMode::ALT_SCREEN) {
        return;
    }

    // Zsh-style right prompts are cursor-positioned suffixes, not soft-wrapped text.
    let grid = terminal.grid_mut();
    let history_size = grid.history_size();
    let screen_lines = grid.screen_lines();
    let current_columns = grid.columns();

    for line in -(history_size as i32)..screen_lines as i32 {
        reanchor_right_aligned_row(&mut grid[Line(line)], current_columns, target_columns);
    }
}

fn reanchor_right_aligned_row(
    row: &mut Row<AlacrittyCell>,
    current_columns: usize,
    target_columns: usize,
) {
    if current_columns == target_columns || current_columns == 0 || target_columns == 0 {
        return;
    }

    if row[Column(current_columns - 1)]
        .flags
        .contains(Flags::WRAPLINE)
    {
        return;
    }

    let Some(last_content) = (0..current_columns)
        .rfind(|column| !row[Column(*column)].is_empty())
        .map(|column| column + 1)
    else {
        return;
    };
    let right_margin = current_columns - last_content;
    if right_margin > MAX_RIGHT_MARGIN {
        return;
    }

    let mut suffix_start = last_content;
    while suffix_start > 0 && !row[Column(suffix_start - 1)].is_empty() {
        suffix_start -= 1;
    }
    let suffix_width = last_content - suffix_start;
    if suffix_width == 0 || suffix_width > MAX_RIGHT_ALIGNED_SUFFIX {
        return;
    }

    let mut prefix_end = suffix_start;
    while prefix_end > 0 && row[Column(prefix_end - 1)].is_empty() {
        prefix_end -= 1;
    }
    let gap_width = suffix_start - prefix_end;
    if prefix_end == 0 || gap_width < MIN_RIGHT_ALIGNED_GAP {
        return;
    }

    let target_end = target_columns.saturating_sub(right_margin);
    let Some(target_start) = target_end.checked_sub(suffix_width) else {
        return;
    };
    if target_start < prefix_end + MIN_RIGHT_ALIGNED_GAP || target_start == suffix_start {
        return;
    }

    let suffix: Vec<_> = (suffix_start..last_content)
        .map(|column| std::mem::take(&mut row[Column(column)]))
        .collect();
    row.grow(target_columns);
    for (offset, cell) in suffix.into_iter().enumerate() {
        row[Column(target_start + offset)] = cell;
    }
}

impl Dimensions for TerminalSize {
    fn total_lines(&self) -> usize {
        self.rows()
    }

    fn screen_lines(&self) -> usize {
        self.rows()
    }

    fn columns(&self) -> usize {
        TerminalSize::columns(*self)
    }
}

fn map_config(config: TerminalConfig) -> AlacrittyConfig {
    AlacrittyConfig {
        scrolling_history: config.scrollback_lines,
        kitty_keyboard: config.kitty_keyboard,
        osc52: match config.osc52 {
            Osc52Mode::Disabled => Osc52::Disabled,
            Osc52Mode::CopyOnly => Osc52::OnlyCopy,
            Osc52Mode::PasteOnly => Osc52::OnlyPaste,
            Osc52Mode::CopyPaste => Osc52::CopyPaste,
        },
        ..AlacrittyConfig::default()
    }
}

fn map_cell(cell: &AlacrittyCell, row: usize, column: usize) -> TerminalCell {
    TerminalCell {
        row,
        column,
        character: cell.c,
        combining_characters: cell.zerowidth().unwrap_or_default().to_vec(),
        foreground: map_color(cell.fg),
        background: map_color(cell.bg),
        underline_color: cell.underline_color().map(map_color),
        underline: map_underline(cell.flags),
        attributes: map_attributes(cell.flags),
        hyperlink: cell.hyperlink().map(|link| Hyperlink {
            id: link.id().to_owned(),
            uri: link.uri().to_owned(),
        }),
    }
}

fn map_cursor(
    shape: AlacrittyCursorShape,
    point: alacritty_terminal::index::Point,
    blinking: bool,
    display_offset: usize,
    size: TerminalSize,
) -> Option<TerminalCursor> {
    if display_offset != 0 || shape == AlacrittyCursorShape::Hidden {
        return None;
    }

    let point = point_to_viewport(display_offset, point)?;
    if point.line >= size.rows() || point.column.0 >= size.columns() {
        return None;
    }

    let shape = match shape {
        AlacrittyCursorShape::Block => CursorShape::Block,
        AlacrittyCursorShape::Underline => CursorShape::Underline,
        AlacrittyCursorShape::Beam => CursorShape::Beam,
        AlacrittyCursorShape::HollowBlock => CursorShape::HollowBlock,
        AlacrittyCursorShape::Hidden => return None,
    };

    Some(TerminalCursor {
        row: point.line,
        column: point.column.0,
        shape,
        blinking,
    })
}

fn map_rgb(color: AlacrittyRgb) -> Rgb {
    Rgb::new(color.r, color.g, color.b)
}

fn map_color(color: AlacrittyColor) -> TerminalColor {
    match color {
        AlacrittyColor::Named(color) => TerminalColor::Named(map_named_color(color)),
        AlacrittyColor::Spec(color) => TerminalColor::Rgb(map_rgb(color)),
        AlacrittyColor::Indexed(index) => TerminalColor::Indexed(index),
    }
}

fn map_named_color(color: AlacrittyNamedColor) -> NamedColor {
    match color {
        AlacrittyNamedColor::Black => NamedColor::Black,
        AlacrittyNamedColor::Red => NamedColor::Red,
        AlacrittyNamedColor::Green => NamedColor::Green,
        AlacrittyNamedColor::Yellow => NamedColor::Yellow,
        AlacrittyNamedColor::Blue => NamedColor::Blue,
        AlacrittyNamedColor::Magenta => NamedColor::Magenta,
        AlacrittyNamedColor::Cyan => NamedColor::Cyan,
        AlacrittyNamedColor::White => NamedColor::White,
        AlacrittyNamedColor::BrightBlack => NamedColor::BrightBlack,
        AlacrittyNamedColor::BrightRed => NamedColor::BrightRed,
        AlacrittyNamedColor::BrightGreen => NamedColor::BrightGreen,
        AlacrittyNamedColor::BrightYellow => NamedColor::BrightYellow,
        AlacrittyNamedColor::BrightBlue => NamedColor::BrightBlue,
        AlacrittyNamedColor::BrightMagenta => NamedColor::BrightMagenta,
        AlacrittyNamedColor::BrightCyan => NamedColor::BrightCyan,
        AlacrittyNamedColor::BrightWhite => NamedColor::BrightWhite,
        AlacrittyNamedColor::Foreground => NamedColor::Foreground,
        AlacrittyNamedColor::Background => NamedColor::Background,
        AlacrittyNamedColor::Cursor => NamedColor::Cursor,
        AlacrittyNamedColor::DimBlack => NamedColor::DimBlack,
        AlacrittyNamedColor::DimRed => NamedColor::DimRed,
        AlacrittyNamedColor::DimGreen => NamedColor::DimGreen,
        AlacrittyNamedColor::DimYellow => NamedColor::DimYellow,
        AlacrittyNamedColor::DimBlue => NamedColor::DimBlue,
        AlacrittyNamedColor::DimMagenta => NamedColor::DimMagenta,
        AlacrittyNamedColor::DimCyan => NamedColor::DimCyan,
        AlacrittyNamedColor::DimWhite => NamedColor::DimWhite,
        AlacrittyNamedColor::BrightForeground => NamedColor::BrightForeground,
        AlacrittyNamedColor::DimForeground => NamedColor::DimForeground,
    }
}

fn map_underline(flags: Flags) -> UnderlineStyle {
    if flags.contains(Flags::UNDERCURL) {
        UnderlineStyle::Curly
    } else if flags.contains(Flags::DOTTED_UNDERLINE) {
        UnderlineStyle::Dotted
    } else if flags.contains(Flags::DASHED_UNDERLINE) {
        UnderlineStyle::Dashed
    } else if flags.contains(Flags::DOUBLE_UNDERLINE) {
        UnderlineStyle::Double
    } else if flags.contains(Flags::UNDERLINE) {
        UnderlineStyle::Single
    } else {
        UnderlineStyle::None
    }
}

fn map_attributes(flags: Flags) -> CellAttributes {
    let mut attributes = CellAttributes::NONE;
    let mappings = [
        (Flags::BOLD, CellAttributes::BOLD),
        (Flags::ITALIC, CellAttributes::ITALIC),
        (Flags::DIM, CellAttributes::DIM),
        (Flags::INVERSE, CellAttributes::INVERSE),
        (Flags::HIDDEN, CellAttributes::HIDDEN),
        (Flags::STRIKEOUT, CellAttributes::STRIKEOUT),
        (Flags::WRAPLINE, CellAttributes::WRAPPED),
        (Flags::WIDE_CHAR, CellAttributes::WIDE),
        (Flags::WIDE_CHAR_SPACER, CellAttributes::WIDE_SPACER),
        (
            Flags::LEADING_WIDE_CHAR_SPACER,
            CellAttributes::LEADING_WIDE_SPACER,
        ),
    ];

    for (source, target) in mappings {
        if flags.contains(source) {
            attributes.insert(target);
        }
    }

    attributes
}

fn map_modes(mode: TermMode) -> TerminalModes {
    let mut modes = TerminalModes::NONE;
    let mappings = [
        (TermMode::SHOW_CURSOR, TerminalModes::SHOW_CURSOR),
        (TermMode::APP_CURSOR, TerminalModes::APPLICATION_CURSOR),
        (TermMode::APP_KEYPAD, TerminalModes::APPLICATION_KEYPAD),
        (
            TermMode::MOUSE_REPORT_CLICK,
            TerminalModes::MOUSE_REPORT_CLICK,
        ),
        (TermMode::BRACKETED_PASTE, TerminalModes::BRACKETED_PASTE),
        (TermMode::SGR_MOUSE, TerminalModes::SGR_MOUSE),
        (TermMode::MOUSE_MOTION, TerminalModes::MOUSE_MOTION),
        (TermMode::LINE_WRAP, TerminalModes::LINE_WRAP),
        (
            TermMode::LINE_FEED_NEW_LINE,
            TerminalModes::LINE_FEED_NEW_LINE,
        ),
        (TermMode::ORIGIN, TerminalModes::ORIGIN),
        (TermMode::INSERT, TerminalModes::INSERT),
        (TermMode::FOCUS_IN_OUT, TerminalModes::FOCUS_REPORTING),
        (TermMode::ALT_SCREEN, TerminalModes::ALTERNATE_SCREEN),
        (TermMode::MOUSE_DRAG, TerminalModes::MOUSE_DRAG),
        (TermMode::UTF8_MOUSE, TerminalModes::UTF8_MOUSE),
        (TermMode::ALTERNATE_SCROLL, TerminalModes::ALTERNATE_SCROLL),
        (TermMode::VI, TerminalModes::VI),
        (
            TermMode::DISAMBIGUATE_ESC_CODES,
            TerminalModes::DISAMBIGUATE_ESCAPE_CODES,
        ),
        (
            TermMode::REPORT_EVENT_TYPES,
            TerminalModes::REPORT_EVENT_TYPES,
        ),
        (
            TermMode::REPORT_ALTERNATE_KEYS,
            TerminalModes::REPORT_ALTERNATE_KEYS,
        ),
        (
            TermMode::REPORT_ALL_KEYS_AS_ESC,
            TerminalModes::REPORT_ALL_KEYS_AS_ESCAPE_CODES,
        ),
        (
            TermMode::REPORT_ASSOCIATED_TEXT,
            TerminalModes::REPORT_ASSOCIATED_TEXT,
        ),
    ];

    for (source, target) in mappings {
        if mode.contains(source) {
            modes.insert(target);
        }
    }

    modes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen::{Clipboard, TerminalPoint, TerminalSelection};

    fn terminal(columns: usize, rows: usize) -> TerminalEngine {
        TerminalEngine::new(columns, rows).expect("valid terminal size")
    }

    fn row_text(snapshot: &TerminalSnapshot, row: usize) -> String {
        snapshot
            .row_text(row)
            .expect("row should exist")
            .trim_end()
            .to_owned()
    }

    #[test]
    fn rejects_sizes_that_cannot_hold_a_terminal_grid() {
        assert_eq!(
            TerminalEngine::new(1, 24).err(),
            Some(InvalidTerminalSize {
                columns: 1,
                rows: 24,
            })
        );
        assert_eq!(
            TerminalEngine::new(80, 0).err(),
            Some(InvalidTerminalSize {
                columns: 80,
                rows: 0,
            })
        );
    }

    #[test]
    fn parses_text_colors_and_attributes() {
        let mut terminal = terminal(8, 2);
        terminal.process(b"\x1b[1;3;38;2;12;34;56;48;5;17mA");

        let snapshot = terminal.snapshot();
        let cell = snapshot.cell(0, 0).expect("first cell");
        assert_eq!(cell.character, 'A');
        assert_eq!(cell.foreground, TerminalColor::Rgb(Rgb::new(12, 34, 56)));
        assert_eq!(cell.background, TerminalColor::Indexed(17));
        assert!(cell.attributes.contains(CellAttributes::BOLD));
        assert!(cell.attributes.contains(CellAttributes::ITALIC));
        assert_eq!(snapshot.cursor.expect("visible cursor").column, 1);
    }

    #[test]
    fn preserves_split_utf8_and_combining_characters() {
        let mut terminal = terminal(8, 2);
        let bytes = "你e\u{301}".as_bytes();
        terminal.process(&bytes[..2]);
        terminal.process(&bytes[2..]);

        let snapshot = terminal.snapshot();
        let wide = snapshot.cell(0, 0).expect("wide cell");
        assert_eq!(wide.character, '你');
        assert!(wide.attributes.contains(CellAttributes::WIDE));
        assert!(
            snapshot
                .cell(0, 1)
                .expect("wide spacer")
                .attributes
                .contains(CellAttributes::WIDE_SPACER)
        );
        let combined = snapshot.cell(0, 2).expect("combined cell");
        assert_eq!(combined.character, 'e');
        assert_eq!(combined.combining_characters, vec!['\u{301}']);
    }

    #[test]
    fn applies_cursor_movement_without_flattening_output() {
        let mut terminal = terminal(8, 2);
        terminal.process(b"abcdef\x1b[3DXY");

        assert_eq!(row_text(&terminal.snapshot(), 0), "abcXYf");
    }

    #[test]
    fn restores_primary_screen_after_leaving_alternate_screen() {
        let mut terminal = terminal(8, 2);
        terminal.process(b"main");
        terminal.process(b"\x1b[?1049h\x1b[Halt");

        let alternate = terminal.snapshot();
        assert_eq!(row_text(&alternate, 0), "alt");
        assert!(alternate.modes.contains(TerminalModes::ALTERNATE_SCREEN));

        terminal.process(b"\x1b[?1049l");
        let primary = terminal.snapshot();
        assert_eq!(row_text(&primary, 0), "main");
        assert!(!primary.modes.contains(TerminalModes::ALTERNATE_SCREEN));
    }

    #[test]
    fn exposes_current_terminal_modes_without_building_a_snapshot() {
        let mut terminal = terminal(8, 2);
        terminal.process(b"\x1b[?1h\x1b[?2004h");

        assert!(terminal.modes().contains(TerminalModes::APPLICATION_CURSOR));
        assert!(terminal.modes().contains(TerminalModes::BRACKETED_PASTE));
    }

    #[test]
    fn exposes_scrollback_and_hides_cursor_while_scrolled() {
        let mut terminal = terminal(6, 2);
        terminal.process(b"one\r\ntwo\r\nthree");

        let bottom = terminal.snapshot();
        assert_eq!(row_text(&bottom, 0), "two");
        assert_eq!(row_text(&bottom, 1), "three");
        assert_eq!(bottom.history_size, 1);

        terminal.scroll(Scroll::Top);
        let top = terminal.snapshot();
        assert_eq!(row_text(&top, 0), "one");
        assert_eq!(row_text(&top, 1), "two");
        assert_eq!(top.display_offset, 1);
        assert_eq!(top.cursor, None);

        terminal.scroll(Scroll::Bottom);
        assert_eq!(terminal.display_offset(), 0);
    }

    #[test]
    fn extracts_forward_and_reverse_multiline_selections() {
        let mut terminal = terminal(8, 3);
        terminal.process(b"alpha\r\nbeta\r\ngamma");
        let snapshot = terminal.snapshot();

        let forward = TerminalSelection::new(TerminalPoint::new(0, 2), TerminalPoint::new(2, 3));
        let reverse = TerminalSelection::new(TerminalPoint::new(2, 3), TerminalPoint::new(0, 2));

        assert_eq!(snapshot.selected_text(forward), "pha\nbeta\ngam");
        assert_eq!(snapshot.selected_text(reverse), "pha\nbeta\ngam");
    }

    #[test]
    fn joins_soft_wrapped_rows_and_trims_terminal_padding() {
        let mut terminal = terminal(5, 2);
        terminal.process(b"abcdef");
        let snapshot = terminal.snapshot();
        let selection = TerminalSelection::new(TerminalPoint::new(0, 0), TerminalPoint::new(1, 5));

        assert_eq!(snapshot.selected_text(selection), "abcdef");
    }

    #[test]
    fn selecting_a_wide_character_spacer_copies_the_character() {
        let mut terminal = terminal(4, 1);
        terminal.process("你x".as_bytes());
        let snapshot = terminal.snapshot();
        let selection = TerminalSelection::new(TerminalPoint::new(0, 1), TerminalPoint::new(0, 2));

        assert_eq!(snapshot.selected_text(selection), "你");
    }

    #[test]
    fn resize_reflows_content_and_updates_dimensions() {
        let mut terminal = terminal(5, 2);
        terminal.process(b"abcdefgh");
        terminal.resize(4, 3).expect("valid resize");

        let snapshot = terminal.snapshot();
        assert_eq!(snapshot.size, TerminalSize::new(4, 3).unwrap());
        let text: String = (0..snapshot.size.rows())
            .map(|row| row_text(&snapshot, row))
            .collect();
        assert_eq!(text, "abcdefgh");
    }

    #[test]
    fn resize_keeps_cursor_positioned_prompt_content_on_its_row() {
        let mut terminal = terminal(97, 8);
        terminal.process(
            "user\r\n❯ \x1b[K\x1b[83C\x1b[44m \u{e606} v3.14.6 \x1b[0m\x1b[94Decho first\r\nFIRST\r\n"
                .as_bytes(),
        );
        terminal.process(
            "user\r\n❯ \x1b[K\x1b[83C\x1b[44m \u{e606} v3.14.6 \x1b[0m\x1b[94Decho second\r\nSECOND\r\n"
                .as_bytes(),
        );

        terminal.resize(67, 8).expect("valid narrow size");
        let narrow_snapshot = terminal.snapshot();
        let narrow_rows: Vec<_> = (0..narrow_snapshot.size.rows())
            .map(|row| narrow_snapshot.row_text(row).unwrap())
            .collect();
        assert!(
            narrow_rows.iter().any(|row| {
                row.starts_with("❯ echo first") && row.trim_end().ends_with("v3.14.6")
            }),
            "first prompt wrapped at the narrow size: {narrow_rows:?}"
        );

        terminal.resize(91, 8).expect("valid restored size");
        let restored_snapshot = terminal.snapshot();
        let restored_rows: Vec<_> = (0..restored_snapshot.size.rows())
            .map(|row| restored_snapshot.row_text(row).unwrap())
            .collect();
        assert!(
            restored_rows.iter().any(|row| {
                row.starts_with("❯ echo first") && row.trim_end().ends_with("v3.14.6")
            }),
            "first prompt was split across rows: {restored_rows:?}"
        );
        assert!(
            restored_rows.iter().any(|row| {
                row.starts_with("❯ echo second") && row.trim_end().ends_with("v3.14.6")
            }),
            "second prompt was split across rows: {restored_rows:?}"
        );
    }

    #[test]
    fn reports_title_and_pty_response_events_in_order() {
        let mut terminal = terminal(8, 2);
        terminal.process(b"\x1b]2;Remote shell\x07\x1b[6n");

        let events = terminal.drain_events();
        let title_index = events
            .iter()
            .position(|event| {
                matches!(
                    event,
                    TerminalEvent::TitleChanged(Some(title)) if title == "Remote shell"
                )
            })
            .expect("title event");
        let response_index = events
            .iter()
            .position(
                |event| matches!(event, TerminalEvent::WriteToPty(bytes) if bytes == b"\x1b[1;1R"),
            )
            .expect("cursor position response");
        assert!(title_index < response_index);
        assert!(terminal.drain_events().is_empty());
    }

    #[test]
    fn preserves_cwd_order_with_other_terminal_events() {
        let mut terminal = terminal(8, 2);
        terminal.process(b"\x1b]2;Before\x07\x1b]7;file://server/home/test\x07\x1b]2;After\x07");

        let events = terminal.drain_events();
        let before = events
            .iter()
            .position(|event| {
                matches!(event, TerminalEvent::TitleChanged(Some(title)) if title == "Before")
            })
            .expect("first title event");
        let cwd = events
            .iter()
            .position(|event| {
                matches!(event, TerminalEvent::WorkingDirectoryChanged(path) if path == "/home/test")
            })
            .expect("cwd event");
        let after = events
            .iter()
            .position(|event| {
                matches!(event, TerminalEvent::TitleChanged(Some(title)) if title == "After")
            })
            .expect("second title event");
        assert!(before < cwd && cwd < after);
    }

    #[test]
    fn emits_decoded_osc52_clipboard_content() {
        let mut terminal = terminal(8, 2);
        terminal.process(b"\x1b]52;c;aGVsbG8=\x07");

        assert!(terminal.drain_events().iter().any(|event| {
            matches!(
                event,
                TerminalEvent::ClipboardStore {
                    clipboard: Clipboard::Clipboard,
                    contents,
                } if contents == "hello"
            )
        }));
    }

    #[test]
    fn formats_ui_backed_terminal_query_responses() {
        let config = TerminalConfig {
            osc52: Osc52Mode::CopyPaste,
            ..TerminalConfig::default()
        };
        let mut terminal = TerminalEngine::with_config(80, 24, config).unwrap();
        terminal.process(b"\x1b]52;c;?\x07\x1b]10;?\x07\x1b[14t");

        let events = terminal.drain_events();
        let clipboard = events
            .iter()
            .find_map(|event| match event {
                TerminalEvent::ClipboardLoad(request) => Some(request),
                _ => None,
            })
            .expect("clipboard query");
        assert_eq!(clipboard.response("hello"), b"\x1b]52;c;aGVsbG8=\x07");

        let color = events
            .iter()
            .find_map(|event| match event {
                TerminalEvent::ColorRequest(request) => Some(request),
                _ => None,
            })
            .expect("color query");
        assert_eq!(color.index, NamedColor::Foreground.palette_index());
        assert_eq!(
            color.response(Rgb::new(1, 2, 3)),
            b"\x1b]10;rgb:0101/0202/0303\x07"
        );

        let text_area = events
            .iter()
            .find_map(|event| match event {
                TerminalEvent::TextAreaSizeRequest(request) => Some(request),
                _ => None,
            })
            .expect("text area query");
        assert_eq!(
            text_area.response(crate::TextAreaSize {
                rows: 24,
                columns: 80,
                cell_width: 9,
                cell_height: 18,
            }),
            b"\x1b[4;432;720t"
        );
    }

    #[test]
    fn exposes_dynamic_palette_overrides() {
        let mut terminal = terminal(8, 2);
        terminal.process(b"\x1b]4;1;rgb:12/34/56\x07");

        assert_eq!(
            terminal.snapshot().palette_overrides.get(1),
            Some(Rgb::new(0x12, 0x34, 0x56))
        );
    }

    #[test]
    fn exposes_hyperlinks_on_renderable_cells() {
        let mut terminal = terminal(8, 2);
        terminal.process(b"\x1b]8;id=docs;https://example.com\x1b\\X\x1b]8;;\x1b\\");

        let hyperlink = terminal
            .snapshot()
            .cell(0, 0)
            .expect("link cell")
            .hyperlink
            .clone()
            .expect("hyperlink");
        assert_eq!(hyperlink.uri, "https://example.com");
        assert!(hyperlink.id.contains("docs"));
    }

    #[test]
    fn damage_is_full_initially_then_tracks_changed_rows() {
        let mut terminal = terminal(8, 2);
        assert_eq!(terminal.take_damage(), TerminalDamage::Full);

        terminal.process(b"x");
        let TerminalDamage::Partial(damage) = terminal.take_damage() else {
            panic!("expected partial damage");
        };
        assert!(damage.iter().any(|range| range.row == 0));
    }
}
