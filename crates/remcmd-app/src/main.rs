use gpui::prelude::*;
use gpui::{
    App, Application, Bounds, Context, IntoElement, Render, Window, WindowBounds, WindowOptions,
    div, px, rgb, size,
};
use remcmd_core::ConnectionProfile;

struct RemCmdApp {
    profile: ConnectionProfile,
}

impl Render for RemCmdApp {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .size_full()
            .bg(rgb(0x111318))
            .text_color(rgb(0xf4f4f5))
            .child(
                div()
                    .w(px(280.0))
                    .h_full()
                    .p_4()
                    .bg(rgb(0x1b1f2a))
                    .child("Connections")
                    .child(
                        div()
                            .mt_3()
                            .p_3()
                            .rounded_sm()
                            .bg(rgb(0x252b38))
                            .child(self.profile.name.clone())
                            .child(format!(
                                "{}@{}:{}",
                                self.profile.username, self.profile.host, self.profile.port
                            )),
                    ),
            )
            .child(div().flex_1().h_full().p_4().child("Terminal placeholder"))
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1200.0), px(760.0)), cx);

        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| {
                cx.new(|_| RemCmdApp {
                    profile: ConnectionProfile::sample(),
                })
            },
        )
        .unwrap();

        cx.activate(true);
    });
}
