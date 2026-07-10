mod text_field;
use text_field::{TextField, bind_text_field_keys};

use std::path::PathBuf;

use gpui::{
    App, Application, Bounds, BoxShadow, Context, Entity, IntoElement, Render, SharedString,
    TitlebarOptions, Window, WindowBackgroundAppearance, WindowBounds, WindowOptions, div, point,
    prelude::*, px, rgb, rgba, size,
};

use remcmd_core::ConnectionProfile;
use remcmd_storage::{default_profiles_path, ensure_profiles_file, load_profiles, save_profiles};

struct RemCmdApp {
    profiles: Vec<ConnectionProfile>,
    selected_profile_id: Option<String>,
    next_profile_number: usize,
    editor: Option<ProfileEditor>,
    form_error: Option<String>,
    profiles_path: PathBuf,
}

#[derive(Clone)]
struct ProfileEditor {
    profile_id: String,
    name: Entity<TextField>,
    host: Entity<TextField>,
    port: Entity<TextField>,
    username: Entity<TextField>,
}

// Application construction and shared data helpers.
impl RemCmdApp {
    fn load(cx: &mut Context<Self>) -> Self {
        let profiles_path = default_profiles_path().expect("failed to resolve profiles path");

        let (profiles, form_error) = match ensure_profiles_file(&profiles_path)
            .and_then(|_| load_profiles(&profiles_path))
        {
            Ok(profiles) => (profiles, None),
            Err(error) => (
                Vec::new(),
                Some(format!("Failed to load profiles: {error}")),
            ),
        };

        let selected_profile_id = profiles.first().map(|profile| profile.id.clone());
        let next_profile_number = profiles
            .iter()
            .filter_map(|profile| profile.id.strip_prefix("demo-")?.parse::<usize>().ok())
            .max()
            .unwrap_or(0)
            + 1;

        let mut app = Self {
            profiles,
            profiles_path,
            selected_profile_id,
            next_profile_number,
            editor: None,
            form_error,
        };

        app.load_editor_for_selected_profile(cx);
        app
    }

    fn selected_profile(&self) -> Option<&ConnectionProfile> {
        let selected_id = self.selected_profile_id.as_ref()?;

        self.profiles
            .iter()
            .find(|profile| &profile.id == selected_id)
    }

    fn persist_profiles(&mut self) {
        if let Err(error) = save_profiles(&self.profiles_path, &self.profiles) {
            self.form_error = Some(format!("Failed to save profiles:\n{error}"));
        }
    }

    fn load_editor_for_selected_profile(&mut self, cx: &mut Context<Self>) {
        let Some(profile) = self.selected_profile().cloned() else {
            self.editor = None;
            return;
        };

        self.editor = Some(ProfileEditor {
            profile_id: profile.id.clone(),
            name: cx.new(|cx| TextField::new(cx, profile.name, "Name")),
            host: cx.new(|cx| TextField::new(cx, profile.host, "Host")),
            port: cx.new(|cx| TextField::new(cx, profile.port.to_string(), "Port")),
            username: cx.new(|cx| TextField::new(cx, profile.username, "Username")),
        });

        self.form_error = None;
    }
}

// User interaction handlers.
impl RemCmdApp {
    fn select_profile(&mut self, profile_id: String, cx: &mut Context<Self>) {
        self.selected_profile_id = Some(profile_id);
        self.load_editor_for_selected_profile(cx);
        cx.notify();
    }

    fn add_profile(&mut self, cx: &mut Context<Self>) {
        let number = self.next_profile_number;

        let profile = ConnectionProfile::new(
            format!("demo-{number}"),
            format!("Demo Server {number}"),
            format!("demo-{number}.example.com"),
            22,
            "ubuntu",
        );

        self.selected_profile_id = Some(profile.id.clone());
        self.profiles.push(profile);
        self.next_profile_number += 1;

        self.load_editor_for_selected_profile(cx);
        self.persist_profiles();

        cx.notify();
    }

    fn delete_selected_profile(&mut self, cx: &mut Context<Self>) {
        let Some(selected_id) = self.selected_profile_id.as_deref() else {
            return;
        };

        let Some(selected_index) = self
            .profiles
            .iter()
            .position(|profile| profile.id == selected_id)
        else {
            self.selected_profile_id = None;
            cx.notify();
            return;
        };

        self.profiles.remove(selected_index);

        self.selected_profile_id = if self.profiles.is_empty() {
            None
        } else if selected_index == 0 {
            Some(self.profiles[0].id.clone())
        } else {
            Some(self.profiles[selected_index - 1].id.clone())
        };

        self.load_editor_for_selected_profile(cx);
        self.persist_profiles();

        cx.notify();
    }

    fn save_editor(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.editor.clone() else {
            return;
        };

        let name = editor.name.read(cx).text();
        let host = editor.host.read(cx).text();
        let port_text = editor.port.read(cx).text();
        let username = editor.username.read(cx).text();

        let Ok(port) = port_text.trim().parse::<u16>() else {
            self.form_error = Some("Port must be a number from 1 to 65535".into());
            cx.notify();
            return;
        };

        if port == 0 {
            self.form_error = Some("Port must be a number from 1 to 65535".into());
            cx.notify();
            return;
        };

        if let Some(profile) = self
            .profiles
            .iter_mut()
            .find(|profile| profile.id == editor.profile_id)
        {
            profile.name = name;
            profile.host = host;
            profile.port = port;
            profile.username = username;
        }

        self.form_error = None;
        self.persist_profiles();

        cx.notify();
    }

    fn cancel_editor(&mut self, cx: &mut Context<Self>) {
        self.load_editor_for_selected_profile(cx);
        cx.notify();
    }
}

// Root rendering entry point and drawing helpers.
impl Render for RemCmdApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let selected_profile = self.selected_profile().cloned();

        div()
            .flex()
            .size_full()
            .bg(rgba(0x18181880))
            .text_color(rgb(0xf4f4f5))
            .child(self.render_sidebar(cx))
            .child(self.render_detail_panel(selected_profile, cx))
    }
}

impl RemCmdApp {
    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut connection_list = div()
            .id("connection_list")
            .flex_1()
            .overflow_y_scroll()
            .mt_3();

        for profile in &self.profiles {
            let profile_id = profile.id.clone();
            let is_selected = self.selected_profile_id.as_ref() == Some(&profile.id);

            let mut profile_item = div()
                .id(SharedString::from(format!("profile-{}", profile.id)))
                .mb_2()
                .p_3()
                .rounded_lg()
                .border_1()
                .border_color(rgba(0xffffff00))
                .cursor_pointer()
                .hover(|this| this.bg(rgba(0xd0d0d033)).border_color(rgba(0xe0e0e080)))
                .child(profile.name.clone())
                .child(profile.address())
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.select_profile(profile_id.clone(), cx);
                }));

            if is_selected {
                profile_item = profile_item
                    .bg(rgba(0xd0d0d033))
                    .border_color(rgba(0xe0e0e080));
            }

            connection_list = connection_list.child(profile_item);
        }

        div()
            .flex()
            .flex_col()
            .w(px(300.0))
            .h_full()
            .bg(rgba(0x18181833))
            .px_4()
            .pb_4()
            .pt(px(52.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .flex_none()
                    .child("Connections")
                    .child(
                        div()
                            .id("add_connection")
                            .px_2()
                            .py_1()
                            .rounded_lg()
                            .bg(rgb(0x3b82f6))
                            .cursor_pointer()
                            .child("Add")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.add_profile(cx);
                            })),
                    ),
            )
            .child(connection_list)
    }

    fn render_detail_panel(
        &self,
        selected_profile: Option<ConnectionProfile>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mut panel = div()
            .flex_1()
            .h_full()
            .px_4()
            .pb_4()
            .pt(px(52.0))
            .bg(rgb(0x181818))
            .border_l_1()
            .border_color(rgba(0xffffff26))
            .shadow(vec![BoxShadow {
                color: rgba(0x0000001f).into(),
                offset: point(px(-1.0), px(0.0)),
                blur_radius: px(4.0),
                spread_radius: px(-2.0),
            }]);

        match selected_profile {
            Some(profile) => {
                let Some(editor) = self.editor.as_ref() else {
                    return panel.child("No editor loaded");
                };

                panel = panel
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(profile.name.clone())
                            .child(
                                div()
                                    .id("delete_connection")
                                    .px_2()
                                    .py_1()
                                    .rounded_lg()
                                    .bg(rgb(0xdc2626))
                                    .cursor_pointer()
                                    .child("Delete")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.delete_selected_profile(cx);
                                    })),
                            ),
                    )
                    .child(self.render_form_row("Name", editor.name.clone()))
                    .child(self.render_form_row("Host", editor.host.clone()))
                    .child(self.render_form_row("Port", editor.port.clone()))
                    .child(self.render_form_row("Username", editor.username.clone()))
                    .when_some(self.form_error.as_ref(), |this, error| {
                        this.child(div().mt_3().text_color(rgb(0xfca5a5)).child(error.clone()))
                    })
                    .child(
                        div()
                            .flex()
                            .mt_6()
                            .gap_2()
                            .child(
                                div()
                                    .id("save_profile")
                                    .px_3()
                                    .py_2()
                                    .rounded_lg()
                                    .bg(rgb(0x2563eb))
                                    .cursor_pointer()
                                    .child("Save")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.save_editor(cx);
                                    })),
                            )
                            .child(
                                div()
                                    .id("cancel_profile")
                                    .px_3()
                                    .py_2()
                                    .rounded_lg()
                                    .bg(rgba(0xffffff18))
                                    .cursor_pointer()
                                    .child("Cancel")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.cancel_editor(cx);
                                    })),
                            ),
                    );
            }
            None => {
                panel = panel.child("No connection selected");
            }
        }

        panel
    }

    fn render_form_row(&self, label: &'static str, field: Entity<TextField>) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .mt_3()
            .child(div().w(px(96.0)).child(label))
            .child(
                div()
                    .flex_1()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgba(0xffffff26))
                    .bg(rgba(0xffffff12))
                    .child(field),
            )
    }
}

// Application startup functions stay outside main so startup remains testable and readable.
fn main_window_options(cx: &App) -> WindowOptions {
    let bounds = Bounds::centered(None, size(px(1200.0), px(800.0)), cx);

    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_background: WindowBackgroundAppearance::Blurred,
        titlebar: Some(TitlebarOptions {
            appears_transparent: true,
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn open_main_window(cx: &mut App) {
    let options = main_window_options(cx);

    cx.open_window(options, |_, cx| cx.new(RemCmdApp::load))
        .expect("failed to open main window");
}

fn launch(cx: &mut App) {
    bind_text_field_keys(cx);
    open_main_window(cx);
    cx.activate(true);
}

fn main() {
    Application::new().run(launch);
}
