use std::ops::Range;

use remcmd_terminal::{
    CellAttributes, CursorShape, NamedColor, PaletteOverrides, Rgb, TerminalColor,
    TerminalSelection, TerminalSnapshot, UnderlineStyle,
};

const DARK_ANSI_COLORS: [ViewColor; 16] = [
    ViewColor::new(0x1e, 0x1e, 0x1e),
    ViewColor::new(0xcd, 0x31, 0x31),
    ViewColor::new(0x0d, 0xbc, 0x79),
    ViewColor::new(0xe5, 0xe5, 0x10),
    ViewColor::new(0x24, 0x72, 0xc8),
    ViewColor::new(0xbc, 0x3f, 0xbc),
    ViewColor::new(0x11, 0xa8, 0xcd),
    ViewColor::new(0xe5, 0xe5, 0xe5),
    ViewColor::new(0x66, 0x66, 0x66),
    ViewColor::new(0xf1, 0x4c, 0x4c),
    ViewColor::new(0x23, 0xd1, 0x8b),
    ViewColor::new(0xf5, 0xf5, 0x43),
    ViewColor::new(0x3b, 0x8e, 0xea),
    ViewColor::new(0xd6, 0x70, 0xd6),
    ViewColor::new(0x29, 0xb8, 0xdb),
    ViewColor::new(0xff, 0xff, 0xff),
];

const LIGHT_ANSI_COLORS: [ViewColor; 16] = [
    ViewColor::new(0x00, 0x00, 0x00),
    ViewColor::new(0xcd, 0x31, 0x31),
    ViewColor::new(0x00, 0x80, 0x00),
    ViewColor::new(0x94, 0x98, 0x00),
    ViewColor::new(0x04, 0x51, 0xa5),
    ViewColor::new(0xbc, 0x05, 0xbc),
    ViewColor::new(0x05, 0x98, 0xbc),
    ViewColor::new(0x55, 0x55, 0x55),
    ViewColor::new(0x66, 0x66, 0x66),
    ViewColor::new(0xcd, 0x31, 0x31),
    ViewColor::new(0x14, 0xce, 0x14),
    ViewColor::new(0xb5, 0xba, 0x00),
    ViewColor::new(0x04, 0x51, 0xa5),
    ViewColor::new(0xbc, 0x05, 0xbc),
    ViewColor::new(0x05, 0x98, 0xbc),
    ViewColor::new(0xa5, 0xa5, 0xa5),
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TerminalPalette {
    pub(crate) foreground: ViewColor,
    pub(crate) background: ViewColor,
    pub(crate) cursor: ViewColor,
    pub(crate) selection: ViewColor,
    ansi: [ViewColor; 16],
}

impl TerminalPalette {
    pub(crate) const fn dark() -> Self {
        Self {
            foreground: ViewColor::new(0xd4, 0xd4, 0xd4),
            background: ViewColor::new(0x18, 0x18, 0x18),
            cursor: ViewColor::new(0xe5, 0xe5, 0xe5),
            selection: ViewColor::new(0x26, 0x4f, 0x78),
            ansi: DARK_ANSI_COLORS,
        }
    }

    pub(crate) const fn light() -> Self {
        Self {
            foreground: ViewColor::new(0x24, 0x29, 0x2f),
            background: ViewColor::new(0xff, 0xff, 0xff),
            cursor: ViewColor::new(0x24, 0x29, 0x2f),
            selection: ViewColor::new(0xad, 0xd6, 0xff),
            ansi: LIGHT_ANSI_COLORS,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ViewColor {
    pub(crate) red: u8,
    pub(crate) green: u8,
    pub(crate) blue: u8,
}

impl ViewColor {
    pub(crate) const fn new(red: u8, green: u8, blue: u8) -> Self {
        Self { red, green, blue }
    }

    pub(crate) const fn hex(self) -> u32 {
        ((self.red as u32) << 16) | ((self.green as u32) << 8) | self.blue as u32
    }

    fn dimmed_against(self, background: Self) -> Self {
        Self::new(
            dim_channel(self.red, background.red),
            dim_channel(self.green, background.green),
            dim_channel(self.blue, background.blue),
        )
    }
}

impl From<Rgb> for ViewColor {
    fn from(color: Rgb) -> Self {
        Self::new(color.red, color.green, color.blue)
    }
}

impl From<ViewColor> for Rgb {
    fn from(color: ViewColor) -> Self {
        Self::new(color.red, color.green, color.blue)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TerminalRunStyle {
    pub(crate) foreground: ViewColor,
    pub(crate) background: ViewColor,
    pub(crate) underline_color: ViewColor,
    pub(crate) bold: bool,
    pub(crate) italic: bool,
    pub(crate) underline: bool,
    pub(crate) strikeout: bool,
    pub(crate) cursor: Option<CursorShape>,
    pub(crate) cursor_color: ViewColor,
    pub(crate) selected: bool,
    pub(crate) selection_background: ViewColor,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TerminalRun {
    pub(crate) range: Range<usize>,
    pub(crate) style: TerminalRunStyle,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TerminalRow {
    pub(crate) text: String,
    pub(crate) runs: Vec<TerminalRun>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TerminalViewModel {
    pub(crate) rows: Vec<TerminalRow>,
}

impl TerminalViewModel {
    #[cfg(test)]
    pub(crate) fn from_snapshot(snapshot: &TerminalSnapshot) -> Self {
        Self::from_snapshot_with_selection(snapshot, None, TerminalPalette::dark())
    }

    pub(crate) fn from_snapshot_with_selection(
        snapshot: &TerminalSnapshot,
        selection: Option<TerminalSelection>,
        palette: TerminalPalette,
    ) -> Self {
        let rows = (0..snapshot.size.rows())
            .map(|row| build_row(snapshot, row, selection, palette))
            .collect();

        Self { rows }
    }
}

pub(crate) fn palette_color(
    snapshot: &TerminalSnapshot,
    index: usize,
    palette: TerminalPalette,
) -> ViewColor {
    resolve_palette_color(&snapshot.palette_overrides, index, palette)
}

fn build_row(
    snapshot: &TerminalSnapshot,
    row: usize,
    selection: Option<TerminalSelection>,
    palette: TerminalPalette,
) -> TerminalRow {
    let mut text = String::with_capacity(snapshot.size.columns());
    let mut runs: Vec<TerminalRun> = Vec::new();

    for column in 0..snapshot.size.columns() {
        let Some(cell) = snapshot.cell(row, column) else {
            continue;
        };

        if cell.attributes.contains(CellAttributes::WIDE_SPACER)
            || cell
                .attributes
                .contains(CellAttributes::LEADING_WIDE_SPACER)
        {
            continue;
        }

        let start = text.len();
        text.push(cell.character);
        text.extend(cell.combining_characters.iter());
        let end = text.len();

        let cursor = snapshot
            .cursor
            .filter(|cursor| cursor.row == row && cursor.column == column)
            .map(|cursor| cursor.shape);
        let selected = cell_is_selected(snapshot, selection, row, column, cell);
        let style = resolve_cell_style(
            snapshot,
            cell.foreground,
            cell.background,
            cell,
            cursor,
            selected,
            palette,
        );

        if let Some(previous) = runs.last_mut()
            && previous.range.end == start
            && previous.style == style
        {
            previous.range.end = end;
        } else {
            runs.push(TerminalRun {
                range: start..end,
                style,
            });
        }
    }

    TerminalRow { text, runs }
}

fn cell_is_selected(
    snapshot: &TerminalSnapshot,
    selection: Option<TerminalSelection>,
    row: usize,
    column: usize,
    cell: &remcmd_terminal::TerminalCell,
) -> bool {
    let Some(selection) = selection else {
        return false;
    };

    if selection.contains(row, column) {
        return true;
    }

    if cell.attributes.contains(CellAttributes::WIDE)
        && column + 1 < snapshot.size.columns()
        && selection.contains(row, column + 1)
    {
        return true;
    }

    column > 0
        && snapshot.cell(row, column - 1).is_some_and(|previous| {
            previous
                .attributes
                .contains(CellAttributes::LEADING_WIDE_SPACER)
                && selection.contains(row, column - 1)
        })
}

fn resolve_cell_style(
    snapshot: &TerminalSnapshot,
    foreground: TerminalColor,
    background: TerminalColor,
    cell: &remcmd_terminal::TerminalCell,
    cursor: Option<CursorShape>,
    selected: bool,
    palette: TerminalPalette,
) -> TerminalRunStyle {
    let mut foreground = resolve_color(snapshot, foreground, palette);
    let mut background = resolve_color(snapshot, background, palette);

    if cell.attributes.contains(CellAttributes::INVERSE) {
        std::mem::swap(&mut foreground, &mut background);
    }

    if cell.attributes.contains(CellAttributes::DIM) {
        foreground = foreground.dimmed_against(background);
    }

    if cell.attributes.contains(CellAttributes::HIDDEN) {
        foreground = background;
    }

    if matches!(cursor, Some(CursorShape::Block | CursorShape::HollowBlock)) {
        foreground = background;
        background = palette_color(snapshot, NamedColor::Cursor.palette_index(), palette);
    }

    TerminalRunStyle {
        foreground,
        background,
        underline_color: cell
            .underline_color
            .map(|color| resolve_color(snapshot, color, palette))
            .unwrap_or(foreground),
        bold: cell.attributes.contains(CellAttributes::BOLD),
        italic: cell.attributes.contains(CellAttributes::ITALIC),
        underline: cell.underline != UnderlineStyle::None,
        strikeout: cell.attributes.contains(CellAttributes::STRIKEOUT),
        cursor,
        cursor_color: palette.cursor,
        selected,
        selection_background: palette.selection,
    }
}

fn resolve_color(
    snapshot: &TerminalSnapshot,
    color: TerminalColor,
    palette: TerminalPalette,
) -> ViewColor {
    match color {
        TerminalColor::Named(color) => palette_color(snapshot, color.palette_index(), palette),
        TerminalColor::Indexed(index) => palette_color(snapshot, usize::from(index), palette),
        TerminalColor::Rgb(color) => color.into(),
    }
}

fn resolve_palette_color(
    overrides: &PaletteOverrides,
    index: usize,
    palette: TerminalPalette,
) -> ViewColor {
    if let Some(color) = overrides.get(index) {
        return color.into();
    }

    match index {
        0..=15 => palette.ansi[index],
        16..=231 => {
            let index = index - 16;
            let red = color_cube_channel(index / 36);
            let green = color_cube_channel((index / 6) % 6);
            let blue = color_cube_channel(index % 6);
            ViewColor::new(red, green, blue)
        }
        232..=255 => {
            let level = 8 + ((index - 232) as u8 * 10);
            ViewColor::new(level, level, level)
        }
        index if index == NamedColor::Foreground.palette_index() => palette.foreground,
        index if index == NamedColor::Background.palette_index() => palette.background,
        index if index == NamedColor::Cursor.palette_index() => palette.cursor,
        index if index == NamedColor::DimBlack.palette_index() => {
            palette.ansi[0].dimmed_against(palette.background)
        }
        index if index == NamedColor::DimRed.palette_index() => {
            palette.ansi[1].dimmed_against(palette.background)
        }
        index if index == NamedColor::DimGreen.palette_index() => {
            palette.ansi[2].dimmed_against(palette.background)
        }
        index if index == NamedColor::DimYellow.palette_index() => {
            palette.ansi[3].dimmed_against(palette.background)
        }
        index if index == NamedColor::DimBlue.palette_index() => {
            palette.ansi[4].dimmed_against(palette.background)
        }
        index if index == NamedColor::DimMagenta.palette_index() => {
            palette.ansi[5].dimmed_against(palette.background)
        }
        index if index == NamedColor::DimCyan.palette_index() => {
            palette.ansi[6].dimmed_against(palette.background)
        }
        index if index == NamedColor::DimWhite.palette_index() => {
            palette.ansi[7].dimmed_against(palette.background)
        }
        index if index == NamedColor::BrightForeground.palette_index() => palette.ansi[15],
        index if index == NamedColor::DimForeground.palette_index() => {
            palette.foreground.dimmed_against(palette.background)
        }
        _ => palette.foreground,
    }
}

fn color_cube_channel(index: usize) -> u8 {
    match index {
        0 => 0,
        1 => 95,
        2 => 135,
        3 => 175,
        4 => 215,
        _ => 255,
    }
}

fn dim_channel(foreground: u8, background: u8) -> u8 {
    ((u16::from(foreground) * 2 + u16::from(background)) / 3) as u8
}

#[cfg(test)]
mod tests {
    use remcmd_terminal::TerminalEngine;

    use super::*;

    fn style_at(row: &TerminalRow, byte_index: usize) -> TerminalRunStyle {
        row.runs
            .iter()
            .find(|run| run.range.contains(&byte_index))
            .expect("styled byte")
            .style
    }

    #[test]
    fn converts_ansi_colors_and_attributes_to_styled_runs() {
        let mut terminal = TerminalEngine::new(6, 1).unwrap();
        terminal.process(b"A\x1b[1;3;31;44mB");

        let model = TerminalViewModel::from_snapshot(&terminal.snapshot());
        let row = &model.rows[0];
        let styled = style_at(row, 1);

        assert_eq!(row.text.len(), 6);
        assert_eq!(styled.foreground, TerminalPalette::dark().ansi[1]);
        assert_eq!(styled.background, TerminalPalette::dark().ansi[4]);
        assert!(styled.bold);
        assert!(styled.italic);
    }

    #[test]
    fn preserves_wide_and_combining_text_without_spacer_glyphs() {
        let mut terminal = TerminalEngine::new(6, 1).unwrap();
        terminal.process("你e\u{301}".as_bytes());

        let model = TerminalViewModel::from_snapshot(&terminal.snapshot());

        assert!(model.rows[0].text.starts_with("你e\u{301}"));
        assert_eq!(
            model.rows[0].text.chars().filter(|ch| *ch == ' ').count(),
            3
        );
    }

    #[test]
    fn applies_dynamic_palette_overrides() {
        let mut terminal = TerminalEngine::new(4, 1).unwrap();
        terminal.process(b"\x1b]4;1;rgb:12/34/56\x07\x1b[31mX");
        let snapshot = terminal.snapshot();

        assert_eq!(
            palette_color(&snapshot, 1, TerminalPalette::dark()),
            ViewColor::new(0x12, 0x34, 0x56)
        );
        assert_eq!(
            style_at(&TerminalViewModel::from_snapshot(&snapshot).rows[0], 0).foreground,
            ViewColor::new(0x12, 0x34, 0x56)
        );
    }

    #[test]
    fn marks_the_visible_cursor_cell() {
        let terminal = TerminalEngine::new(4, 1).unwrap();
        let model = TerminalViewModel::from_snapshot(&terminal.snapshot());
        let cursor_style = style_at(&model.rows[0], 0);

        assert_eq!(cursor_style.cursor, Some(CursorShape::Block));
        assert_eq!(cursor_style.background, TerminalPalette::dark().cursor);
    }

    #[test]
    fn light_palette_uses_white_defaults_and_dark_text() {
        let mut terminal = TerminalEngine::new(4, 1).unwrap();
        terminal.process(b"A");
        let palette = TerminalPalette::light();
        let model =
            TerminalViewModel::from_snapshot_with_selection(&terminal.snapshot(), None, palette);
        let style = style_at(&model.rows[0], 0);

        assert_eq!(style.foreground, palette.foreground);
        assert_eq!(style.background, palette.background);
        assert_eq!(palette.background, ViewColor::new(0xff, 0xff, 0xff));
    }

    #[test]
    fn splits_runs_at_selection_boundaries() {
        let mut terminal = TerminalEngine::new(4, 1).unwrap();
        terminal.process(b"abcd");
        let snapshot = terminal.snapshot();
        let selection = TerminalSelection::new(
            remcmd_terminal::TerminalPoint::new(0, 1),
            remcmd_terminal::TerminalPoint::new(0, 3),
        );
        let model = TerminalViewModel::from_snapshot_with_selection(
            &snapshot,
            Some(selection),
            TerminalPalette::dark(),
        );

        assert!(!style_at(&model.rows[0], 0).selected);
        assert!(style_at(&model.rows[0], 1).selected);
        assert!(style_at(&model.rows[0], 2).selected);
        assert!(!style_at(&model.rows[0], 3).selected);
    }

    #[test]
    fn selecting_a_wide_spacer_highlights_the_wide_glyph() {
        let mut terminal = TerminalEngine::new(4, 1).unwrap();
        terminal.process("你x".as_bytes());
        let snapshot = terminal.snapshot();
        let selection = TerminalSelection::new(
            remcmd_terminal::TerminalPoint::new(0, 1),
            remcmd_terminal::TerminalPoint::new(0, 2),
        );
        let model = TerminalViewModel::from_snapshot_with_selection(
            &snapshot,
            Some(selection),
            TerminalPalette::dark(),
        );

        assert!(style_at(&model.rows[0], 0).selected);
    }
}
