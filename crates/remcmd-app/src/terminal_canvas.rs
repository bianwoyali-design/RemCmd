use std::{cell::RefCell, collections::BTreeSet, rc::Rc, sync::Arc};

use gpui::{
    App, BorderStyle, Bounds, ContentMask, FontStyle, FontWeight, Pixels, ShapedLine, SharedString,
    TextStyle, Window, fill, outline, point, px, rgb, size,
};

use remcmd_terminal::{
    CursorShape, PaletteOverrides, TerminalCursor, TerminalDamage, TerminalSelection, TerminalSize,
    TerminalSnapshot,
};

use crate::terminal_view::{
    TerminalPalette, TerminalRow, TerminalRunStyle, TerminalViewModel, ViewColor,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TerminalCellMetrics {
    pub(crate) width: f32,
    pub(crate) height: f32,
}

impl TerminalCellMetrics {
    pub(crate) fn measure(window: &mut Window) -> Self {
        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window
            .text_system()
            .shape_line("M".into(), font_size, &[style.to_run(1)], None);

        Self {
            width: f32::from(line.width),
            height: f32::from(window.line_height()),
        }
    }
}

pub(crate) struct TerminalCanvasFrame {
    rows: Vec<Arc<PreparedRow>>,
    default_background: ViewColor,
    metrics: TerminalCellMetrics,
}

pub(crate) struct TerminalCanvasInput<'a> {
    pub(crate) cache: &'a Rc<RefCell<TerminalCanvasCache>>,
    pub(crate) snapshot: &'a TerminalSnapshot,
    pub(crate) damage: TerminalDamage,
    pub(crate) selection: Option<TerminalSelection>,
    pub(crate) palette: TerminalPalette,
    pub(crate) font_family: SharedString,
    pub(crate) font_size: f32,
    pub(crate) metrics: TerminalCellMetrics,
}

#[derive(Default)]
pub(crate) struct TerminalCanvasCache {
    key: Option<TerminalCanvasCacheKey>,
    cursor: Option<TerminalCursor>,
    rows: Vec<Arc<PreparedRow>>,
}

#[derive(Clone, Debug, PartialEq)]
struct TerminalCanvasCacheKey {
    size: TerminalSize,
    display_offset: usize,
    palette_overrides: PaletteOverrides,
    palette: TerminalPalette,
    selection: Option<TerminalSelection>,
    font_family: SharedString,
    font_size: f32,
    metrics: TerminalCellMetrics,
}

#[derive(Clone)]
struct PreparedRow {
    cells: Vec<PreparedCell>,
}

#[derive(Clone)]
struct PreparedCell {
    column: usize,
    width: usize,
    line: Option<ShapedLine>,
    style: TerminalRunStyle,
}

impl TerminalCanvasFrame {
    pub(crate) fn prepare(input: TerminalCanvasInput<'_>, window: &mut Window) -> Self {
        let TerminalCanvasInput {
            cache,
            snapshot,
            damage,
            selection,
            palette,
            font_family,
            font_size: terminal_font_size,
            metrics,
        } = input;
        let key = TerminalCanvasCacheKey {
            size: snapshot.size,
            display_offset: snapshot.display_offset,
            palette_overrides: snapshot.palette_overrides.clone(),
            palette,
            selection,
            font_family,
            font_size: terminal_font_size,
            metrics,
        };
        let mut cache = cache.borrow_mut();
        let base_style = window.text_style().clone();
        let font_size = base_style.font_size.to_pixels(window.rem_size());
        let full_rebuild = cache.key.as_ref() != Some(&key)
            || matches!(damage, TerminalDamage::Full)
            || cache.rows.len() != snapshot.size.rows();
        let mut damaged_rows = BTreeSet::new();
        if let TerminalDamage::Partial(ranges) = damage {
            damaged_rows.extend(
                ranges
                    .into_iter()
                    .map(|range| range.row)
                    .filter(|row| *row < snapshot.size.rows()),
            );
        }
        if cache.cursor != snapshot.cursor {
            if let Some(cursor) = cache.cursor {
                damaged_rows.insert(cursor.row);
            }
            if let Some(cursor) = snapshot.cursor {
                damaged_rows.insert(cursor.row);
            }
        }

        if full_rebuild {
            cache.rows = (0..snapshot.size.rows())
                .map(|row| {
                    Arc::new(prepare_row(
                        TerminalViewModel::row_from_snapshot(snapshot, row, selection, palette),
                        &base_style,
                        font_size,
                        window,
                    ))
                })
                .collect();
        } else {
            for row in damaged_rows {
                if let Some(slot) = cache.rows.get_mut(row) {
                    *slot = Arc::new(prepare_row(
                        TerminalViewModel::row_from_snapshot(snapshot, row, selection, palette),
                        &base_style,
                        font_size,
                        window,
                    ));
                }
            }
        }
        cache.key = Some(key);
        cache.cursor = snapshot.cursor;

        Self {
            rows: cache.rows.clone(),
            default_background: palette.background,
            metrics,
        }
    }

    pub(crate) fn paint(self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        self.paint_backgrounds(bounds, window);

        for (row, prepared_row) in self.rows.iter().enumerate() {
            for cell in &prepared_row.cells {
                let cell_bounds = self.cell_bounds(bounds, row, cell);

                if let Some(line) = cell.line.as_ref() {
                    let available_width = f32::from(cell_bounds.size.width);
                    let glyph_offset = (available_width - f32::from(line.width)) / 2.0;
                    let origin = point(cell_bounds.left() + px(glyph_offset), cell_bounds.top());

                    window.with_content_mask(
                        Some(ContentMask {
                            bounds: cell_bounds,
                        }),
                        |window| {
                            line.paint(origin, px(self.metrics.height), window, cx)
                                .expect("terminal cell glyph should paint");
                        },
                    );
                }

                paint_decorations(cell_bounds, cell.style, window);
            }
        }
    }

    fn paint_backgrounds(&self, bounds: Bounds<Pixels>, window: &mut Window) {
        for (row, prepared_row) in self.rows.iter().enumerate() {
            let mut active_run: Option<(usize, usize, ViewColor)> = None;

            for cell in &prepared_row.cells {
                let background = effective_background(cell.style);
                let cell_end = cell.column + cell.width;

                if let Some((_start, end, color)) = active_run.as_mut()
                    && *end == cell.column
                    && *color == background
                {
                    *end = cell_end;
                    continue;
                }

                if let Some(run) = active_run.take() {
                    self.paint_background_run(bounds, row, run, window);
                }
                active_run = Some((cell.column, cell_end, background));
            }

            if let Some(run) = active_run {
                self.paint_background_run(bounds, row, run, window);
            }
        }
    }

    fn paint_background_run(
        &self,
        bounds: Bounds<Pixels>,
        row: usize,
        (start, end, color): (usize, usize, ViewColor),
        window: &mut Window,
    ) {
        if color == self.default_background {
            return;
        }

        window.paint_quad(fill(
            Bounds::new(
                point(
                    bounds.left() + px(start as f32 * self.metrics.width),
                    bounds.top() + px(row as f32 * self.metrics.height),
                ),
                size(
                    px((end - start) as f32 * self.metrics.width),
                    px(self.metrics.height),
                ),
            ),
            rgb(color.hex()),
        ));
    }

    fn cell_bounds(
        &self,
        bounds: Bounds<Pixels>,
        row: usize,
        cell: &PreparedCell,
    ) -> Bounds<Pixels> {
        Bounds::new(
            point(
                bounds.left() + px(cell.column as f32 * self.metrics.width),
                bounds.top() + px(row as f32 * self.metrics.height),
            ),
            size(
                px(cell.width as f32 * self.metrics.width),
                px(self.metrics.height),
            ),
        )
    }
}

fn prepare_row(
    row: TerminalRow,
    base_style: &TextStyle,
    font_size: Pixels,
    window: &mut Window,
) -> PreparedRow {
    PreparedRow {
        cells: row
            .cells
            .into_iter()
            .map(|cell| {
                let line = if cell.text.chars().all(|character| character == ' ') {
                    None
                } else {
                    let style = text_style_for_cell(base_style, cell.style);
                    let run = style.to_run(cell.text.len());
                    Some(
                        window
                            .text_system()
                            .shape_line(cell.text.into(), font_size, &[run], None),
                    )
                };

                PreparedCell {
                    column: cell.column,
                    width: cell.width,
                    line,
                    style: cell.style,
                }
            })
            .collect(),
    }
}

fn text_style_for_cell(base: &TextStyle, cell: TerminalRunStyle) -> TextStyle {
    let mut style = base.clone();
    style.color = rgb(cell.foreground.hex()).into();
    if cell.bold {
        style.font_weight = FontWeight::BOLD;
    }
    if cell.italic {
        style.font_style = FontStyle::Italic;
    }
    style.background_color = None;
    style.underline = None;
    style.strikethrough = None;
    style
}

fn effective_background(style: TerminalRunStyle) -> ViewColor {
    if style.selected {
        style.selection_background
    } else {
        style.background
    }
}

fn paint_decorations(bounds: Bounds<Pixels>, style: TerminalRunStyle, window: &mut Window) {
    if style.underline {
        window.paint_quad(fill(
            Bounds::new(
                point(bounds.left(), bounds.bottom() - px(2.0)),
                size(bounds.size.width, px(1.0)),
            ),
            rgb(style.underline_color.hex()),
        ));
    }

    if style.strikeout {
        window.paint_quad(fill(
            Bounds::new(
                point(bounds.left(), bounds.top() + bounds.size.height / 2.0),
                size(bounds.size.width, px(1.0)),
            ),
            rgb(style.foreground.hex()),
        ));
    }

    let cursor_color = rgb(style.cursor_color.hex());
    match style.cursor {
        Some(CursorShape::Underline) => window.paint_quad(fill(
            Bounds::new(
                point(bounds.left(), bounds.bottom() - px(2.0)),
                size(bounds.size.width, px(2.0)),
            ),
            cursor_color,
        )),
        Some(CursorShape::Beam) => window.paint_quad(fill(
            Bounds::new(bounds.origin, size(px(2.0), bounds.size.height)),
            cursor_color,
        )),
        Some(CursorShape::HollowBlock) => {
            window.paint_quad(outline(bounds, cursor_color, BorderStyle::default()))
        }
        Some(CursorShape::Block) | None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_bounds_use_terminal_columns_instead_of_glyph_widths() {
        let frame = TerminalCanvasFrame {
            rows: Vec::new(),
            default_background: ViewColor::new(0, 0, 0),
            metrics: TerminalCellMetrics {
                width: 8.25,
                height: 19.0,
            },
        };
        let cell = PreparedCell {
            column: 10,
            width: 2,
            line: None,
            style: TerminalRunStyle {
                foreground: ViewColor::new(255, 255, 255),
                background: ViewColor::new(0, 0, 0),
                underline_color: ViewColor::new(255, 255, 255),
                bold: false,
                italic: false,
                underline: false,
                strikeout: false,
                cursor: None,
                cursor_color: ViewColor::new(255, 255, 255),
                selected: false,
                selection_background: ViewColor::new(1, 1, 1),
            },
        };

        let bounds = frame.cell_bounds(
            Bounds::new(point(px(5.0), px(7.0)), size(px(400.0), px(200.0))),
            3,
            &cell,
        );

        assert_eq!(bounds.left(), px(87.5));
        assert_eq!(bounds.top(), px(64.0));
        assert_eq!(bounds.size.width, px(16.5));
    }
}
