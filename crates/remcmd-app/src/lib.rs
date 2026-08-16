mod app;
mod file_editor;
mod i18n;
mod icons;
#[cfg(target_os = "macos")]
mod macos_symbols;
mod pane_layout;
#[cfg(target_os = "macos")]
mod private_key_picker;
mod ssh_runtime;
mod terminal_canvas;
mod terminal_input;
mod terminal_view;
mod text_field;
mod theme;
mod windows_chrome;

pub use app::run;
