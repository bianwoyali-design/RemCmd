use super::{
    App, Application, Bounds, CancelCredential, CancelHostKeyVerification, CancelProfileEditor,
    CancelQuickCommand, CancelSettingsSelector, CancelSftpCreate, DiagnosticLevel, Diagnostics,
    DiagnosticsGlobal, KeyBinding, LanguageMode, Localizer, RemCmdApp, RemCmdAssets,
    RemCmdMainWindow, SaveProfileEditor, SshRuntime, SubmitCredential, SubmitQuickCommand,
    SubmitSftpCreate, TRAFFIC_LIGHT_INSET_X, TRAFFIC_LIGHT_INSET_Y, TitlebarOptions,
    WindowBackgroundAppearance, WindowBounds, WindowHandle, WindowOptions, bind_file_editor_keys,
    bind_text_field_keys, configure_application_menu, default_log_directory,
    fallback_log_directory, point, px, size,
};
use gpui::prelude::*;
use std::path::Path;

#[cfg(target_os = "macos")]
pub(super) fn register_macos_sf_mono(cx: &mut App) {
    const FONT_DIRECTORIES: [&str; 2] = [
        "/System/Applications/Utilities/Terminal.app/Contents/Resources/Fonts",
        "/Applications/Utilities/Terminal.app/Contents/Resources/Fonts",
    ];
    const FONT_FILES: [&str; 12] = [
        "SF-Mono-Regular.otf",
        "SF-Mono-RegularItalic.otf",
        "SF-Mono-Medium.otf",
        "SF-Mono-MediumItalic.otf",
        "SF-Mono-Semibold.otf",
        "SF-Mono-SemiboldItalic.otf",
        "SF-Mono-Bold.otf",
        "SF-Mono-BoldItalic.otf",
        "SF-Mono-Light.otf",
        "SF-Mono-LightItalic.otf",
        "SF-Mono-Heavy.otf",
        "SF-Mono-HeavyItalic.otf",
    ];

    let Some(directory) = FONT_DIRECTORIES
        .iter()
        .map(Path::new)
        .find(|directory| directory.join(FONT_FILES[0]).is_file())
    else {
        return;
    };
    let fonts = FONT_FILES
        .iter()
        .filter_map(|file| std::fs::read(directory.join(file)).ok())
        .map(std::borrow::Cow::Owned)
        .collect::<Vec<_>>();
    if !fonts.is_empty() {
        let _ = cx.text_system().add_fonts(fonts);
    }
}

#[cfg(not(target_os = "macos"))]
pub(super) fn register_macos_sf_mono(_: &mut App) {}

pub(super) fn main_window_titlebar() -> TitlebarOptions {
    #[cfg(target_os = "macos")]
    {
        TitlebarOptions {
            appears_transparent: true,
            traffic_light_position: Some(point(
                px(TRAFFIC_LIGHT_INSET_X),
                px(TRAFFIC_LIGHT_INSET_Y),
            )),
            ..Default::default()
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        TitlebarOptions {
            title: Some("RemCmd".into()),
            appears_transparent: cfg!(target_os = "windows"),
            ..Default::default()
        }
    }
}

pub(super) fn main_window_options(cx: &App) -> WindowOptions {
    let bounds = Bounds::centered(None, size(px(1200.0), px(800.0)), cx);

    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(size(px(720.0), px(480.0))),
        window_background: WindowBackgroundAppearance::Blurred,
        titlebar: Some(main_window_titlebar()),
        ..Default::default()
    }
}

pub(super) fn open_main_window(cx: &mut App) -> WindowHandle<RemCmdApp> {
    let options = main_window_options(cx);

    cx.open_window(options, |window, cx| {
        cx.new(|cx| RemCmdApp::load(window, cx))
    })
    .expect("failed to open main window")
}

pub(super) fn bind_credential_prompt_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("enter", SubmitCredential, Some("CredentialPrompt")),
        KeyBinding::new("escape", CancelCredential, Some("CredentialPrompt")),
    ]);
}

pub(super) fn bind_host_key_prompt_keys(cx: &mut App) {
    cx.bind_keys([KeyBinding::new(
        "escape",
        CancelHostKeyVerification,
        Some("HostKeyPrompt"),
    )]);
}

pub(super) fn bind_settings_selector_keys(cx: &mut App) {
    cx.bind_keys([KeyBinding::new(
        "escape",
        CancelSettingsSelector,
        Some("Settings"),
    )]);
}

pub(super) fn bind_sftp_create_prompt_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("enter", SubmitSftpCreate, Some("SftpCreatePrompt")),
        KeyBinding::new("escape", CancelSftpCreate, Some("SftpCreatePrompt")),
    ]);
}

pub(super) fn bind_quick_command_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("enter", SubmitQuickCommand, Some("QuickCommandPrompt")),
        KeyBinding::new("escape", CancelQuickCommand, Some("QuickCommandPrompt")),
    ]);
}

pub(super) fn bind_profile_editor_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-s", SaveProfileEditor, Some("ProfileEditor")),
        KeyBinding::new("escape", CancelProfileEditor, Some("ProfileEditor")),
    ]);
    #[cfg(target_os = "windows")]
    cx.bind_keys([KeyBinding::new(
        "ctrl-s",
        SaveProfileEditor,
        Some("ProfileEditor"),
    )]);
}

pub(super) fn launch(cx: &mut App) {
    let log_directory = default_log_directory().unwrap_or_else(|_| fallback_log_directory());
    let diagnostics = Diagnostics::initialize(log_directory);
    let diagnostic_store = diagnostics.store();
    diagnostic_store.record(
        DiagnosticLevel::Info,
        "app.lifecycle",
        "RemCmd started",
        [("version".into(), env!("CARGO_PKG_VERSION").into())],
    );
    cx.set_global(DiagnosticsGlobal(diagnostic_store));
    cx.set_global(SshRuntime::new().expect("failed to create SSH runtime"));
    register_macos_sf_mono(cx);

    bind_text_field_keys(cx);
    bind_file_editor_keys(cx);
    bind_credential_prompt_keys(cx);
    bind_host_key_prompt_keys(cx);
    bind_settings_selector_keys(cx);
    bind_sftp_create_prompt_keys(cx);
    bind_quick_command_keys(cx);
    bind_profile_editor_keys(cx);
    let main_window = open_main_window(cx);
    let language_mode = main_window
        .update(cx, |this, _, _| this.language_mode)
        .unwrap_or(LanguageMode::System);
    cx.set_global(RemCmdMainWindow(main_window));
    configure_application_menu(cx, &Localizer::new(language_mode));
    cx.activate(true);
}

pub(super) fn reopen_main_window(cx: &mut App) {
    let main_window = open_main_window(cx);
    cx.set_global(RemCmdMainWindow(main_window));
    cx.activate(true);
}

pub fn run() {
    let application = Application::new().with_assets(RemCmdAssets);
    application.on_reopen(reopen_main_window);
    application.run(launch);
}
