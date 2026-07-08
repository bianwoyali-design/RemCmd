use gpui::{
    App, Application, Bounds, Context, IntoElement, Render, Window, WindowBounds, WindowOptions,
    div, px, rgb, size,
};
use gpui::{SharedString, prelude::*};
use remcmd_core::ConnectionProfile;

struct RemCmdApp {
    profiles: Vec<ConnectionProfile>,
    selected_profile_id: Option<String>,
    next_profile_number: usize,
}

impl RemCmdApp {
    fn selected_profile(&self) -> Option<&ConnectionProfile> {
        let selected_id = self.selected_profile_id.as_ref()?;

        self.profiles
            .iter()
            .find(|profile| &profile.id == selected_id)
    }

    fn select_profile(&mut self, profile_id: String, cx: &mut Context<Self>) {
        self.selected_profile_id = Some(profile_id);
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

        cx.notify();
    }
}

impl Render for RemCmdApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let selected_profile = self.selected_profile().cloned();

        div()
            .flex()
            .size_full()
            .bg(rgb(0x111318))
            .text_color(rgb(0xf4f4f5))
            .child(self.render_sidebar(cx))
            .child(self.render_detail_panel(selected_profile, cx))
    }
}

impl RemCmdApp {
    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut sidebar = div().w(px(300.0)).h_full().p_4().bg(rgb(0x1b1f2a)).child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child("Connections")
                .child(
                    div()
                        .id("add-connection")
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .bg(rgb(0x3b82f6))
                        .cursor_pointer()
                        .child("Add")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.add_profile(cx);
                        })),
                ),
        );

        for profile in &self.profiles {
            let profile_id = profile.id.clone();
            let is_selected = self.selected_profile_id.as_ref() == Some(&profile.id);

            let bg_color = if is_selected {
                rgb(0x334155)
            } else {
                rgb(0x252b38)
            };

            sidebar = sidebar.child(
                div()
                    .id(SharedString::from(format!("profile-{}", profile_id)))
                    .mt_3()
                    .p_3()
                    .rounded_md()
                    .bg(bg_color)
                    .cursor_pointer()
                    .child(profile.name.clone())
                    .child(profile.address())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select_profile(profile_id.clone(), cx);
                    })),
            );
        }

        sidebar
    }

    fn render_detail_panel(
        &self,
        selected_profile: Option<ConnectionProfile>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mut panel = div().flex_1().h_full().p_4();

        match selected_profile {
            Some(profile) => {
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
                                    .rounded_md()
                                    .bg(rgb(0xdc2626))
                                    .cursor_pointer()
                                    .child("Delete")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.delete_selected_profile(cx);
                                    })),
                            ),
                    )
                    .child(div().mt_4().child(format!("Host: {}", profile.host)))
                    .child(div().mt_2().child(format!("Port: {}", profile.port)))
                    .child(
                        div()
                            .mt_2()
                            .child(format!("Username: {}", profile.username)),
                    )
                    .child(div().mt_6().child("Terminal placeholder"));
            }
            None => {
                panel = panel.child("No connection selected");
            }
        }

        panel
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1200.0), px(800.0)), cx);

        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| {
                cx.new(|_| RemCmdApp {
                    profiles: ConnectionProfile::samples(),
                    selected_profile_id: Some("local-dev".into()),
                    next_profile_number: 3,
                })
            },
        )
        .unwrap();

        cx.activate(true);
    });
}
