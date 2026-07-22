use std::ops::Range;

use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, Element, ElementId, ElementInputHandler,
    Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable, GlobalElementId, KeyBinding,
    LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point,
    Render, ScrollHandle, ShapedLine, SharedString, Style, TextRun, UTF16Selection, UnderlineStyle,
    Window, actions, div, fill, point, prelude::*, px, relative, size,
};
use unicode_segmentation::UnicodeSegmentation;

use crate::theme::Theme;

const LINE_HEIGHT: f32 = 22.0;
const FONT_SIZE: f32 = 13.0;
const PADDING_X: f32 = 12.0;
const PADDING_Y: f32 = 8.0;
const APPROXIMATE_CELL_WIDTH: f32 = 8.0;
const UNDO_LIMIT: usize = 100;

#[cfg(target_os = "macos")]
const EDITOR_FONT_FAMILY: &str = "SF Mono";
#[cfg(target_os = "windows")]
const EDITOR_FONT_FAMILY: &str = "Consolas";
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const EDITOR_FONT_FAMILY: &str = "DejaVu Sans Mono";

actions!(
    file_editor,
    [
        Backspace,
        Delete,
        Left,
        Right,
        Up,
        Down,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        SelectAll,
        Home,
        End,
        Enter,
        Tab,
        Paste,
        Cut,
        Copy,
        Undo,
        Redo,
        Save
    ]
);

#[derive(Clone, Copy, Debug)]
pub enum FileEditorEvent {
    SaveRequested,
}

#[derive(Clone)]
struct EditorSnapshot {
    content: String,
    selection: Range<usize>,
    selection_reversed: bool,
}

#[derive(Clone)]
struct EditorLineLayout {
    range: Range<usize>,
    origin: Point<Pixels>,
    layout: ShapedLine,
}

pub struct FileEditor {
    focus_handle: FocusHandle,
    content: String,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    preferred_column: Option<usize>,
    last_bounds: Option<Bounds<Pixels>>,
    last_layouts: Vec<EditorLineLayout>,
    is_selecting: bool,
    reveal_cursor: bool,
    scroll_handle: ScrollHandle,
    undo_stack: Vec<EditorSnapshot>,
    redo_stack: Vec<EditorSnapshot>,
}

impl FileEditor {
    pub fn new(cx: &mut Context<Self>, content: String) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            content,
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            preferred_column: None,
            last_bounds: None,
            last_layouts: Vec::new(),
            is_selecting: false,
            reveal_cursor: false,
            scroll_handle: ScrollHandle::new(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
        }
    }

    pub fn text(&self) -> &str {
        &self.content
    }

    pub fn replace_all(&mut self, content: String, cx: &mut Context<Self>) {
        self.content = content;
        self.selected_range = 0..0;
        self.selection_reversed = false;
        self.marked_range = None;
        self.preferred_column = None;
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.reveal_cursor = false;
        self.scroll_handle.set_offset(point(px(0.0), px(0.0)));
        cx.notify();
    }

    fn snapshot(&self) -> EditorSnapshot {
        EditorSnapshot {
            content: self.content.clone(),
            selection: self.selected_range.clone(),
            selection_reversed: self.selection_reversed,
        }
    }

    fn restore(&mut self, snapshot: EditorSnapshot, cx: &mut Context<Self>) {
        self.content = snapshot.content;
        self.selected_range = snapshot.selection;
        self.selection_reversed = snapshot.selection_reversed;
        self.marked_range = None;
        self.preferred_column = None;
        self.reveal_cursor = true;
        cx.notify();
    }

    fn push_undo(&mut self) {
        self.undo_stack.push(self.snapshot());
        if self.undo_stack.len() > UNDO_LIMIT {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.previous_boundary(self.cursor_offset()), cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.next_boundary(self.cursor_offset()), cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        self.preferred_column = None;
        let offset = if self.selected_range.is_empty() {
            self.previous_boundary(self.cursor_offset())
        } else {
            self.selected_range.start
        };
        self.move_to(offset, cx);
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        self.preferred_column = None;
        let offset = if self.selected_range.is_empty() {
            self.next_boundary(self.cursor_offset())
        } else {
            self.selected_range.end
        };
        self.move_to(offset, cx);
    }

    fn up(&mut self, _: &Up, _: &mut Window, cx: &mut Context<Self>) {
        let offset = self.vertical_offset(-1);
        self.move_to(offset, cx);
    }

    fn down(&mut self, _: &Down, _: &mut Window, cx: &mut Context<Self>) {
        let offset = self.vertical_offset(1);
        self.move_to(offset, cx);
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.preferred_column = None;
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.preferred_column = None;
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    fn select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        let offset = self.vertical_offset(-1);
        self.select_to(offset, cx);
    }

    fn select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        let offset = self.vertical_offset(1);
        self.select_to(offset, cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.selected_range = 0..self.content.len();
        self.selection_reversed = false;
        self.preferred_column = None;
        cx.notify();
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.preferred_column = None;
        self.move_to(self.line_start(self.cursor_offset()), cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.preferred_column = None;
        self.move_to(self.line_end(self.cursor_offset()), cx);
    }

    fn enter(&mut self, _: &Enter, window: &mut Window, cx: &mut Context<Self>) {
        self.replace_text_in_range(None, "\n", window, cx);
    }

    fn tab(&mut self, _: &Tab, window: &mut Window, cx: &mut Context<Self>) {
        self.replace_text_in_range(None, "    ", window, cx);
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_text_in_range(None, &text, window, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_owned(),
            ));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            self.copy(&Copy, window, cx);
            self.replace_text_in_range(None, "", window, cx);
        }
    }

    fn undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(snapshot) = self.undo_stack.pop() {
            self.redo_stack.push(self.snapshot());
            self.restore(snapshot, cx);
        }
    }

    fn redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(snapshot) = self.redo_stack.pop() {
            self.undo_stack.push(self.snapshot());
            self.restore(snapshot, cx);
        }
    }

    fn save(&mut self, _: &Save, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(FileEditorEvent::SaveRequested);
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_handle.focus(window);
        self.is_selecting = true;
        self.preferred_column = None;
        let offset = self.index_for_mouse_position(event.position);
        if event.modifiers.shift {
            self.select_to(offset, cx);
        } else {
            self.move_to(offset, cx);
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_selecting {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        self.selection_reversed = false;
        self.reveal_cursor = true;
        cx.notify();
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.selection_reversed {
            self.selected_range.start = offset;
        } else {
            self.selected_range.end = offset;
        }
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        self.reveal_cursor = true;
        cx.notify();
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn line_start(&self, offset: usize) -> usize {
        self.content[..offset]
            .rfind('\n')
            .map_or(0, |newline| newline + 1)
    }

    fn line_end(&self, offset: usize) -> usize {
        self.content[offset..]
            .find('\n')
            .map_or(self.content.len(), |newline| offset + newline)
    }

    fn vertical_offset(&mut self, direction: isize) -> usize {
        let cursor = self.cursor_offset();
        let start = self.line_start(cursor);
        let column = *self
            .preferred_column
            .get_or_insert_with(|| self.content[start..cursor].graphemes(true).count());

        let target_start = if direction < 0 {
            if start == 0 {
                return 0;
            }
            self.line_start(start - 1)
        } else {
            let end = self.line_end(cursor);
            if end == self.content.len() {
                return self.content.len();
            }
            end + 1
        };
        let target_end = self.line_end(target_start);
        self.content[target_start..target_end]
            .grapheme_indices(true)
            .nth(column)
            .map_or(target_end, |(offset, _)| target_start + offset)
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        let Some(bounds) = self.last_bounds else {
            return 0;
        };
        if position.y <= bounds.top() + px(PADDING_Y) {
            return 0;
        }
        if position.y >= bounds.bottom() - px(PADDING_Y) {
            return self.content.len();
        }

        self.last_layouts
            .iter()
            .find(|line| {
                position.y >= line.origin.y && position.y < line.origin.y + px(LINE_HEIGHT)
            })
            .map(|line| {
                let offset = line.layout.closest_index_for_x(position.x - line.origin.x);
                line.range.start + offset.min(line.range.len())
            })
            .unwrap_or_else(|| {
                if position.y < bounds.top() {
                    0
                } else {
                    self.content.len()
                }
            })
    }

    fn cursor_position(&self) -> Option<Point<Pixels>> {
        let cursor = self.cursor_offset();
        self.last_layouts
            .iter()
            .find(|line| cursor >= line.range.start && cursor <= line.range.end)
            .map(|line| {
                point(
                    line.origin.x + line.layout.x_for_index(cursor - line.range.start),
                    line.origin.y,
                )
            })
    }

    fn reveal_cursor_if_needed(&mut self) {
        if !self.reveal_cursor {
            return;
        }
        let Some(cursor) = self.cursor_position() else {
            return;
        };
        let viewport = self.scroll_handle.bounds();
        if viewport.size.width <= px(0.0) || viewport.size.height <= px(0.0) {
            return;
        }

        let mut offset = self.scroll_handle.offset();
        if cursor.y < viewport.top() + px(PADDING_Y) {
            offset.y += viewport.top() + px(PADDING_Y) - cursor.y;
        } else if cursor.y + px(LINE_HEIGHT) > viewport.bottom() - px(PADDING_Y) {
            offset.y -= cursor.y + px(LINE_HEIGHT) - viewport.bottom() + px(PADDING_Y);
        }
        if cursor.x < viewport.left() + px(PADDING_X) {
            offset.x += viewport.left() + px(PADDING_X) - cursor.x;
        } else if cursor.x + px(APPROXIMATE_CELL_WIDTH) > viewport.right() - px(PADDING_X) {
            offset.x -= cursor.x + px(APPROXIMATE_CELL_WIDTH) - viewport.right() + px(PADDING_X);
        }

        let max = self.scroll_handle.max_offset();
        offset.x = offset.x.clamp(-max.width, px(0.0));
        offset.y = offset.y.clamp(-max.height, px(0.0));
        self.scroll_handle.set_offset(offset);
        self.reveal_cursor = false;
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8_offset = 0;
        let mut utf16_count = 0;
        for character in self.content.chars() {
            if utf16_count >= offset {
                break;
            }
            utf16_count += character.len_utf16();
            utf8_offset += character.len_utf8();
        }
        utf8_offset
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        self.content[..offset].encode_utf16().count()
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range.start)..self.offset_from_utf16(range.end)
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .rev()
            .find_map(|(index, _)| (index < offset).then_some(index))
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .find_map(|(index, _)| (index > offset).then_some(index))
            .unwrap_or(self.content.len())
    }

    fn line_ranges(&self) -> Vec<Range<usize>> {
        line_ranges(&self.content)
    }

    fn content_width(&self) -> f32 {
        self.content
            .split('\n')
            .map(|line| {
                line.graphemes(true)
                    .map(|grapheme| match grapheme {
                        "\t" => APPROXIMATE_CELL_WIDTH * 4.0,
                        _ if grapheme.is_ascii() => APPROXIMATE_CELL_WIDTH,
                        _ => FONT_SIZE,
                    })
                    .sum::<f32>()
            })
            .fold(0.0, f32::max)
            + PADDING_X * 2.0
    }
}

impl EventEmitter<FileEditorEvent> for FileEditor {}

impl EntityInputHandler for FileEditor {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_owned())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        if self.content[range.clone()] == *new_text {
            return;
        }

        self.push_undo();
        self.content.replace_range(range.clone(), new_text);
        let cursor = range.start + new_text.len();
        self.selected_range = cursor..cursor;
        self.selection_reversed = false;
        self.marked_range = None;
        self.preferred_column = None;
        self.reveal_cursor = true;
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        self.push_undo();
        self.content.replace_range(range.clone(), new_text);
        self.marked_range =
            (!new_text.is_empty()).then(|| range.start..range.start + new_text.len());
        self.selected_range = new_selected_range_utf16
            .as_ref()
            .map(|selection| self.range_from_utf16(selection))
            .map(|selection| range.start + selection.start..range.start + selection.end)
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());
        self.selection_reversed = false;
        self.preferred_column = None;
        self.reveal_cursor = true;
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.range_from_utf16(&range_utf16);
        let offset = range.start;
        let line = self
            .last_layouts
            .iter()
            .find(|line| offset >= line.range.start && offset <= line.range.end)?;
        let x = line.origin.x + line.layout.x_for_index(offset - line.range.start);
        Some(Bounds::new(
            point(x, line.origin.y),
            size(px(1.5), px(LINE_HEIGHT)),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.offset_to_utf16(self.index_for_mouse_position(point)))
    }
}

struct FileEditorElement {
    editor: Entity<FileEditor>,
}

struct PrepaintState {
    lines: Vec<EditorLineLayout>,
    cursor: Option<PaintQuad>,
}

impl IntoElement for FileEditorElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for FileEditorElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let editor = self.editor.read(cx);
        let mut style = Style::default();
        style.size.width = relative(1.0).into();
        style.size.height =
            px(editor.line_ranges().len() as f32 * LINE_HEIGHT + PADDING_Y * 2.0).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let editor = self.editor.read(cx);
        let theme = *cx.global::<Theme>();
        let style = window.text_style();
        let font_size = px(FONT_SIZE);
        let selected = editor.selected_range.clone();
        let marked = editor.marked_range.clone();
        let cursor_offset = editor.cursor_offset();
        let mut cursor = None;
        let mut lines = Vec::new();

        for (index, range) in editor.line_ranges().into_iter().enumerate() {
            let text: SharedString = editor.content[range.clone()].to_owned().into();
            let runs = text_runs_for_line(
                &style,
                range.clone(),
                &selected,
                marked.as_ref(),
                theme.selection_bg,
            );
            let layout = window
                .text_system()
                .shape_line(text, font_size, &runs, None);
            let origin = point(
                bounds.left() + px(PADDING_X),
                bounds.top() + px(PADDING_Y + index as f32 * LINE_HEIGHT),
            );
            if cursor_offset >= range.start && cursor_offset <= range.end {
                cursor = Some(fill(
                    Bounds::new(
                        point(
                            origin.x + layout.x_for_index(cursor_offset - range.start),
                            origin.y,
                        ),
                        size(px(1.5), px(LINE_HEIGHT)),
                    ),
                    theme.input_cursor,
                ));
            }
            lines.push(EditorLineLayout {
                range,
                origin,
                layout,
            });
        }

        PrepaintState { lines, cursor }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.editor.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.editor.clone()),
            cx,
        );

        for line in &prepaint.lines {
            line.layout
                .paint_background(line.origin, px(LINE_HEIGHT), window, cx)
                .expect("file editor background should paint");
            line.layout
                .paint(line.origin, px(LINE_HEIGHT), window, cx)
                .expect("file editor text should paint");
        }
        if focus_handle.is_focused(window)
            && let Some(cursor) = prepaint.cursor.take()
        {
            window.paint_quad(cursor);
        }

        self.editor.update(cx, |editor, _| {
            editor.last_bounds = Some(bounds);
            editor.last_layouts.clone_from(&prepaint.lines);
            editor.reveal_cursor_if_needed();
        });
    }
}

impl Render for FileEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let width = self.content_width().max(1.0);
        div()
            .id("remote_file_editor")
            .key_context("FileEditor")
            .track_focus(&self.focus_handle)
            .cursor(CursorStyle::IBeam)
            .size_full()
            .overflow_scroll()
            .track_scroll(&self.scroll_handle)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_up))
            .on_action(cx.listener(Self::select_down))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::enter))
            .on_action(cx.listener(Self::tab))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_action(cx.listener(Self::save))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .font_family(EDITOR_FONT_FAMILY)
            .text_size(px(FONT_SIZE))
            .line_height(px(LINE_HEIGHT))
            .child(div().w(px(width)).min_w_full().child(FileEditorElement {
                editor: cx.entity(),
            }))
    }
}

impl Focusable for FileEditor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn line_ranges(content: &str) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut start = 0;
    for (index, character) in content.char_indices() {
        if character == '\n' {
            ranges.push(start..index);
            start = index + 1;
        }
    }
    ranges.push(start..content.len());
    ranges
}

fn text_runs_for_line(
    style: &gpui::TextStyle,
    line: Range<usize>,
    selection: &Range<usize>,
    marked: Option<&Range<usize>>,
    selection_background: gpui::Hsla,
) -> Vec<TextRun> {
    let mut boundaries = vec![line.start, line.end];
    for range in [Some(selection), marked].into_iter().flatten() {
        boundaries.push(range.start.clamp(line.start, line.end));
        boundaries.push(range.end.clamp(line.start, line.end));
    }
    boundaries.sort_unstable();
    boundaries.dedup();

    boundaries
        .windows(2)
        .filter_map(|boundary| {
            let range = boundary[0]..boundary[1];
            (!range.is_empty()).then(|| TextRun {
                len: range.len(),
                font: style.font(),
                color: style.color,
                background_color: ranges_overlap(&range, selection).then_some(selection_background),
                underline: marked
                    .filter(|marked| ranges_overlap(&range, marked))
                    .map(|_| UnderlineStyle {
                        color: Some(style.color),
                        thickness: px(1.0),
                        wavy: false,
                    }),
                strikethrough: None,
            })
        })
        .collect()
}

fn ranges_overlap(left: &Range<usize>, right: &Range<usize>) -> bool {
    left.start < right.end && right.start < left.end
}

pub fn bind_file_editor_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, Some("FileEditor")),
        KeyBinding::new("delete", Delete, Some("FileEditor")),
        KeyBinding::new("left", Left, Some("FileEditor")),
        KeyBinding::new("right", Right, Some("FileEditor")),
        KeyBinding::new("up", Up, Some("FileEditor")),
        KeyBinding::new("down", Down, Some("FileEditor")),
        KeyBinding::new("shift-left", SelectLeft, Some("FileEditor")),
        KeyBinding::new("shift-right", SelectRight, Some("FileEditor")),
        KeyBinding::new("shift-up", SelectUp, Some("FileEditor")),
        KeyBinding::new("shift-down", SelectDown, Some("FileEditor")),
        KeyBinding::new("cmd-a", SelectAll, Some("FileEditor")),
        KeyBinding::new("ctrl-a", SelectAll, Some("FileEditor")),
        KeyBinding::new("home", Home, Some("FileEditor")),
        KeyBinding::new("end", End, Some("FileEditor")),
        KeyBinding::new("enter", Enter, Some("FileEditor")),
        KeyBinding::new("tab", Tab, Some("FileEditor")),
        KeyBinding::new("cmd-v", Paste, Some("FileEditor")),
        KeyBinding::new("ctrl-v", Paste, Some("FileEditor")),
        KeyBinding::new("cmd-c", Copy, Some("FileEditor")),
        KeyBinding::new("ctrl-c", Copy, Some("FileEditor")),
        KeyBinding::new("cmd-x", Cut, Some("FileEditor")),
        KeyBinding::new("ctrl-x", Cut, Some("FileEditor")),
        KeyBinding::new("cmd-z", Undo, Some("FileEditor")),
        KeyBinding::new("ctrl-z", Undo, Some("FileEditor")),
        KeyBinding::new("cmd-shift-z", Redo, Some("FileEditor")),
        KeyBinding::new("ctrl-y", Redo, Some("FileEditor")),
        KeyBinding::new("cmd-s", Save, Some("FileEditor")),
        KeyBinding::new("ctrl-s", Save, Some("FileEditor")),
    ]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_ranges_include_empty_trailing_line() {
        assert_eq!(line_ranges("first\nsecond\n"), vec![0..5, 6..12, 13..13]);
        assert_eq!(line_ranges(""), vec![0..0]);
    }

    #[test]
    fn overlap_excludes_touching_ranges() {
        assert!(ranges_overlap(&(1..3), &(2..4)));
        assert!(!ranges_overlap(&(1..3), &(3..5)));
    }
}
