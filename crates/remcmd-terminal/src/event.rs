use std::collections::VecDeque;
use std::fmt::{self, Debug, Formatter};
use std::sync::{Arc, Mutex, MutexGuard};

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::term::ClipboardType;
use alacritty_terminal::vte::ansi::Rgb as AlacrittyRgb;

use crate::screen::{Clipboard, Rgb, TextAreaSize};

#[derive(Clone)]
pub struct ClipboardLoadRequest {
    pub clipboard: Clipboard,
    formatter: Arc<dyn Fn(&str) -> String + Send + Sync + 'static>,
}

impl ClipboardLoadRequest {
    pub fn response(&self, contents: &str) -> Vec<u8> {
        (self.formatter)(contents).into_bytes()
    }
}

impl Debug for ClipboardLoadRequest {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ClipboardLoadRequest")
            .field("clipboard", &self.clipboard)
            .finish_non_exhaustive()
    }
}

#[derive(Clone)]
pub struct ColorRequest {
    pub index: usize,
    formatter: Arc<dyn Fn(AlacrittyRgb) -> String + Send + Sync + 'static>,
}

impl ColorRequest {
    pub fn response(&self, color: Rgb) -> Vec<u8> {
        (self.formatter)(AlacrittyRgb {
            r: color.red,
            g: color.green,
            b: color.blue,
        })
        .into_bytes()
    }
}

impl Debug for ColorRequest {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ColorRequest")
            .field("index", &self.index)
            .finish_non_exhaustive()
    }
}

#[derive(Clone)]
pub struct TextAreaSizeRequest {
    formatter: Arc<dyn Fn(WindowSize) -> String + Send + Sync + 'static>,
}

impl TextAreaSizeRequest {
    pub fn response(&self, size: TextAreaSize) -> Vec<u8> {
        (self.formatter)(WindowSize {
            num_lines: size.rows,
            num_cols: size.columns,
            cell_width: size.cell_width,
            cell_height: size.cell_height,
        })
        .into_bytes()
    }
}

impl Debug for TextAreaSizeRequest {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TextAreaSizeRequest")
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug)]
pub enum TerminalEvent {
    MouseCursorDirty,
    TitleChanged(Option<String>),
    WorkingDirectoryChanged(String),
    ClipboardStore {
        clipboard: Clipboard,
        contents: String,
    },
    ClipboardLoad(ClipboardLoadRequest),
    ColorRequest(ColorRequest),
    WriteToPty(Vec<u8>),
    TextAreaSizeRequest(TextAreaSizeRequest),
    CursorBlinkingChanged,
    Wakeup,
    Bell,
    ExitRequested,
    ChildExited(Option<i32>),
}

#[derive(Clone, Default)]
pub(crate) struct EventQueue {
    events: Arc<Mutex<VecDeque<TerminalEvent>>>,
}

impl EventQueue {
    pub(crate) fn drain(&self) -> Vec<TerminalEvent> {
        let mut events = lock_unpoisoned(&self.events);
        events.drain(..).collect()
    }

    pub(crate) fn push(&self, event: TerminalEvent) {
        lock_unpoisoned(&self.events).push_back(event);
    }
}

impl EventListener for EventQueue {
    fn send_event(&self, event: Event) {
        self.push(TerminalEvent::from(event));
    }
}

impl From<Event> for TerminalEvent {
    fn from(event: Event) -> Self {
        match event {
            Event::MouseCursorDirty => Self::MouseCursorDirty,
            Event::Title(title) => Self::TitleChanged(Some(title)),
            Event::ResetTitle => Self::TitleChanged(None),
            Event::ClipboardStore(clipboard, contents) => Self::ClipboardStore {
                clipboard: map_clipboard(clipboard),
                contents,
            },
            Event::ClipboardLoad(clipboard, formatter) => {
                Self::ClipboardLoad(ClipboardLoadRequest {
                    clipboard: map_clipboard(clipboard),
                    formatter,
                })
            }
            Event::ColorRequest(index, formatter) => {
                Self::ColorRequest(ColorRequest { index, formatter })
            }
            Event::PtyWrite(contents) => Self::WriteToPty(contents.into_bytes()),
            Event::TextAreaSizeRequest(formatter) => {
                Self::TextAreaSizeRequest(TextAreaSizeRequest { formatter })
            }
            Event::CursorBlinkingChange => Self::CursorBlinkingChanged,
            Event::Wakeup => Self::Wakeup,
            Event::Bell => Self::Bell,
            Event::Exit => Self::ExitRequested,
            Event::ChildExit(status) => Self::ChildExited(status.code()),
        }
    }
}

fn map_clipboard(clipboard: ClipboardType) -> Clipboard {
    match clipboard {
        ClipboardType::Clipboard => Clipboard::Clipboard,
        ClipboardType::Selection => Clipboard::Selection,
    }
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
