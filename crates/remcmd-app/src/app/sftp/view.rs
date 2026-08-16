use super::sftp_state::{
    SftpAvailability, SftpBrowserPlacement, SftpContextMenu, SftpCreateKind, SftpTransferState,
    SftpTreeRow, format_remote_size, remote_breadcrumbs, remote_parent_path,
};
use super::{
    AnyElement, CancelSftpCreate, Context, FontWeight, IconName, IconTone, MouseButton, Range,
    RemCmdApp, RemoteFileKind, SessionId, SessionState, SftpTransferDirection, SharedString,
    SubmitSftpCreate, TextButtonTone, UI_MONOSPACE_FONT_FAMILY, Window, div, icon, px, text_button,
    uniform_list,
};
use gpui::prelude::*;

impl RemCmdApp {
    pub(super) fn render_sftp_context_menu(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(menu_state) = self.sftp_context_menu.as_ref() else {
            return div().into_any_element();
        };
        let session_id = menu_state.session_id;
        let placement = menu_state.placement;
        let entries = self.selected_sftp_entries(session_id, placement);
        let connected = self
            .session(session_id)
            .is_some_and(|session| session.connection_state == SessionState::Connected);
        let single_file = (entries.len() == 1 && entries[0].kind == RemoteFileKind::File)
            .then(|| entries[0].clone());
        let can_download = connected
            && entries.iter().any(|entry| {
                matches!(entry.kind, RemoteFileKind::File | RemoteFileKind::Directory)
            });
        let viewport = window.viewport_size();
        let left = f32::from(menu_state.position.x)
            .min((f32::from(viewport.width) - 196.0).max(8.0))
            .max(8.0);
        let top = f32::from(menu_state.position.y)
            .min((f32::from(viewport.height) - 286.0).max(8.0))
            .max(8.0);

        let mut new_file = self.render_context_menu_item(
            "sftp-context-new-file",
            IconName::File,
            self.tr("sftp-new-file"),
            IconTone::Default,
            connected,
        );
        if connected {
            new_file = new_file.on_click(cx.listener(move |this, _, window, cx| {
                this.open_sftp_create_prompt(
                    session_id,
                    placement,
                    SftpCreateKind::File,
                    window,
                    cx,
                );
            }));
        }
        let mut new_folder = self.render_context_menu_item(
            "sftp-context-new-folder",
            IconName::Folder,
            self.tr("sftp-new-folder"),
            IconTone::Default,
            connected,
        );
        if connected {
            new_folder = new_folder.on_click(cx.listener(move |this, _, window, cx| {
                this.open_sftp_create_prompt(
                    session_id,
                    placement,
                    SftpCreateKind::Directory,
                    window,
                    cx,
                );
            }));
        }
        let mut copy_path = self.render_context_menu_item(
            "sftp-context-copy-path",
            IconName::Copy,
            self.tr("sftp-copy-path"),
            IconTone::Default,
            !entries.is_empty(),
        );
        if !entries.is_empty() {
            copy_path = copy_path.on_click(cx.listener(move |this, _, _, cx| {
                this.copy_selected_sftp_paths(session_id, placement, cx);
            }));
        }
        let mut view = self.render_context_menu_item(
            "sftp-context-view",
            IconName::View,
            self.tr("sftp-view"),
            IconTone::Default,
            single_file.is_some(),
        );
        if single_file.is_some() {
            view = view.on_click(cx.listener(move |this, _, _, cx| {
                this.open_selected_sftp_file(session_id, placement, false, cx);
            }));
        }
        let mut edit = self.render_context_menu_item(
            "sftp-context-edit",
            IconName::Edit,
            self.tr("sftp-edit"),
            IconTone::Default,
            single_file.is_some(),
        );
        if single_file.is_some() {
            edit = edit.on_click(cx.listener(move |this, _, _, cx| {
                this.open_selected_sftp_file(session_id, placement, true, cx);
            }));
        }
        let mut download = self.render_context_menu_item(
            "sftp-context-download",
            IconName::Download,
            self.tr("sftp-download"),
            IconTone::Default,
            can_download,
        );
        if can_download {
            download = download.on_click(cx.listener(move |this, _, _, cx| {
                this.download_selected_sftp_entries(session_id, placement, cx);
            }));
        }
        let mut delete = self.render_context_menu_item(
            "sftp-context-delete",
            IconName::Delete,
            self.tr("common-delete"),
            IconTone::Danger,
            connected && !entries.is_empty(),
        );
        if connected && !entries.is_empty() {
            delete = delete.on_click(cx.listener(move |this, _, window, cx| {
                this.delete_selected_sftp_entries(session_id, placement, window, cx);
            }));
        }

        self.glass_floating_surface()
            .id("sftp_context_menu")
            .absolute()
            .left(px(left))
            .top(px(top))
            .w(px(188.0))
            .flex()
            .flex_col()
            .p_1()
            .occlude()
            .child(new_file)
            .child(new_folder)
            .child(self.render_context_menu_separator())
            .child(copy_path)
            .child(view)
            .child(edit)
            .child(download)
            .child(self.render_context_menu_separator())
            .child(delete)
            .into_any_element()
    }

    pub(super) fn render_sftp_create_prompt(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(prompt) = self.sftp_create_prompt.as_ref() else {
            return div().into_any_element();
        };
        let title = match prompt.kind {
            SftpCreateKind::File => self.tr("sftp-new-file"),
            SftpCreateKind::Directory => self.tr("sftp-new-folder"),
        };
        let input = prompt.input.clone();
        let create = text_button(
            "submit_sftp_create",
            self.tr("common-create"),
            TextButtonTone::Primary,
            true,
            &self.theme,
        )
        .on_click(cx.listener(|this, _, _, cx| this.submit_sftp_create(cx)));
        let cancel = text_button(
            "cancel_sftp_create",
            self.tr("common-cancel"),
            TextButtonTone::Secondary,
            true,
            &self.theme,
        )
        .on_click(cx.listener(|this, _, _, cx| {
            this.sftp_create_prompt = None;
            cx.notify();
        }));

        div()
            .id("sftp_create_prompt_overlay")
            .absolute()
            .top_0()
            .right_0()
            .bottom_0()
            .left_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(self.theme.overlay_bg)
            .occlude()
            .key_context("SftpCreatePrompt")
            .on_action(cx.listener(|this, _: &SubmitSftpCreate, _, cx| this.submit_sftp_create(cx)))
            .on_action(cx.listener(|this, _: &CancelSftpCreate, _, cx| {
                this.sftp_create_prompt = None;
                cx.notify();
            }))
            .child(
                self.glass_floating_surface()
                    .w(px(360.0))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p_4()
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
                    .child(input)
                    .when_some(prompt.error.as_ref(), |this, error| {
                        this.child(
                            div()
                                .text_sm()
                                .text_color(self.theme.error_text)
                                .child(error.clone()),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap_2()
                            .child(cancel)
                            .child(create),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn render_sftp_browser(
        &self,
        session_id: SessionId,
        placement: SftpBrowserPlacement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(session) = self.session(session_id) else {
            return div().into_any_element();
        };
        if placement == SftpBrowserPlacement::Center && session.sftp.file.is_some() {
            return self.render_sftp_file(session_id, cx);
        }
        if session.connection_state == SessionState::Connected {
            match &session.sftp_availability {
                SftpAvailability::Checking => {
                    return self.render_sftp_availability_hint(
                        self.tr("sftp-checking"),
                        self.tr("sftp-checking-detail"),
                    );
                }
                SftpAvailability::Unavailable(message) => {
                    return self.render_sftp_availability_hint(
                        self.tr("sftp-unavailable"),
                        message.clone(),
                    );
                }
                SftpAvailability::ScpOnly(message) => {
                    return self.render_scp_upload_fallback(
                        session_id,
                        placement,
                        message.clone(),
                        cx,
                    );
                }
                SftpAvailability::Available => {}
            }
        }
        let browser_state = session.sftp_browser(placement);
        let path = browser_state.path.clone();
        let tree = placement == SftpBrowserPlacement::Sidebar;
        let entry_count = browser_state.visible_rows(tree).len();
        let selected_entries = browser_state.selected_entries();
        let scroll_handle = browser_state.scroll_handle.clone();
        let loading = browser_state.loading;
        let loaded = browser_state.loaded;
        let error = browser_state.error.clone();
        let connected = session.connection_state == SessionState::Connected;
        let can_go_up = connected && remote_parent_path(&path).is_some() && !loading;
        let element_suffix = placement.element_suffix();
        let list_id = SharedString::from(format!("sftp_directory_entries_{element_suffix}"));

        let list = if !loaded && loading {
            div()
                .id(list_id)
                .flex()
                .flex_1()
                .min_h(px(0.0))
                .w_full()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(self.theme.text_muted)
                .child(self.tr("sftp-loading-files"))
                .into_any_element()
        } else if loaded && entry_count == 0 {
            div()
                .id(list_id)
                .flex()
                .flex_1()
                .min_h(px(0.0))
                .w_full()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(self.theme.text_muted)
                .child(self.tr("sftp-empty-directory"))
                .into_any_element()
        } else {
            div()
                .flex()
                .flex_1()
                .min_h(px(0.0))
                .w_full()
                .overflow_hidden()
                .child(
                    uniform_list(
                        list_id,
                        entry_count,
                        cx.processor(move |this, range: Range<usize>, _, cx| {
                            this.render_sftp_entry_rows(session_id, placement, range, cx)
                        }),
                    )
                    .size_full()
                    .track_scroll(scroll_handle),
                )
                .into_any_element()
        };

        let mut parent_button = self.render_icon_button(
            SharedString::from(format!("sftp_parent_directory_{element_suffix}")),
            IconName::ArrowUp,
            self.tr("sftp-parent-directory"),
            IconTone::Default,
            can_go_up,
        );
        if can_go_up {
            parent_button = parent_button.on_click(cx.listener(move |this, _, _, cx| {
                this.open_parent_remote_directory(placement, cx);
            }));
        }
        let can_refresh = connected && !loading;
        let mut refresh_button = self.render_icon_button(
            SharedString::from(format!("sftp_refresh_directory_{element_suffix}")),
            IconName::Reconnect,
            self.tr("common-refresh"),
            IconTone::Default,
            can_refresh,
        );
        if can_refresh {
            refresh_button = refresh_button.on_click(cx.listener(move |this, _, _, cx| {
                this.refresh_active_sftp_directory(placement, cx);
            }));
        }
        let can_upload = connected && !loading;
        let mut upload_button = self.render_icon_button(
            SharedString::from(format!("sftp_upload_{element_suffix}")),
            IconName::Upload,
            self.tr("sftp-upload-files"),
            IconTone::Default,
            can_upload,
        );
        if can_upload {
            upload_button = upload_button.on_click(cx.listener(move |this, _, _, cx| {
                this.choose_sftp_uploads(session_id, placement, cx);
            }));
        }
        let can_download = connected
            && !loading
            && selected_entries.iter().any(|entry| {
                matches!(entry.kind, RemoteFileKind::File | RemoteFileKind::Directory)
            });
        let mut download_button = self.render_icon_button(
            SharedString::from(format!("sftp_download_selected_{element_suffix}")),
            IconName::Download,
            self.tr("sftp-download-selected"),
            IconTone::Default,
            can_download,
        );
        if can_download {
            download_button = download_button.on_click(cx.listener(move |this, _, _, cx| {
                this.download_selected_sftp_entries(session_id, placement, cx);
            }));
        }
        let can_delete = connected && !loading && !selected_entries.is_empty();
        let browser_background = if placement == SftpBrowserPlacement::Sidebar {
            self.theme.transparent
        } else {
            self.theme.panel_bg
        };
        let mut delete_button = self.render_icon_button(
            SharedString::from(format!("sftp_delete_selected_{element_suffix}")),
            IconName::Delete,
            self.tr("sftp-delete-selected"),
            IconTone::Danger,
            can_delete,
        );
        if can_delete {
            delete_button = delete_button.on_click(cx.listener(move |this, _, window, cx| {
                this.delete_selected_sftp_entries(session_id, placement, window, cx);
            }));
        }

        let mut browser = div()
            .id(SharedString::from(format!("sftp_browser_{element_suffix}")))
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .mt_4()
            .overflow_hidden()
            .rounded_md()
            .border_1()
            .border_color(self.theme.border)
            .bg(browser_background)
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    if let Some(session) = this.session_mut(session_id) {
                        let browser = session.sftp_browser_mut(placement);
                        browser.selected_paths.clear();
                        browser.selection_anchor = None;
                    }
                    this.sftp_context_menu = Some(SftpContextMenu {
                        session_id,
                        placement,
                        position: event.position,
                    });
                    cx.notify();
                }),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap_1()
                    .h(px(40.0))
                    .px_2()
                    .border_b_1()
                    .border_color(self.theme.border)
                    .child(parent_button)
                    .child(refresh_button)
                    .child(upload_button)
                    .child(download_button)
                    .child(delete_button)
                    .child(div().flex_1())
                    .when(loading && loaded, |this| {
                        this.child(
                            div()
                                .flex_none()
                                .text_sm()
                                .text_color(self.theme.text_muted)
                                .child(self.tr("common-loading")),
                        )
                    }),
            );

        if let Some(error) = error {
            browser = browser.child(
                div()
                    .flex_none()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(self.theme.border)
                    .bg(self.theme.control_bg)
                    .text_sm()
                    .text_color(self.theme.error_text)
                    .child(error),
            );
        }

        browser
            .child(list)
            .child(self.render_sftp_transfer_queue(session_id, placement, cx))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .h(px(32.0))
                    .min_w(px(0.0))
                    .px_2()
                    .overflow_hidden()
                    .border_t_1()
                    .border_color(self.theme.border)
                    .bg(self.theme.surface_bg)
                    .child(self.render_sftp_breadcrumbs(session_id, placement, &path, cx)),
            )
            .into_any_element()
    }

    pub(super) fn render_sftp_availability_hint(
        &self,
        title: impl Into<SharedString>,
        message: impl Into<SharedString>,
    ) -> AnyElement {
        div()
            .flex()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .flex_col()
            .items_center()
            .justify_center()
            .gap_2()
            .px_4()
            .text_center()
            .child(self.render_sidebar_icon(IconName::Folder, 22.0))
            .child(div().font_weight(FontWeight::MEDIUM).child(title.into()))
            .child(
                div()
                    .max_w(px(420.0))
                    .text_sm()
                    .text_color(self.theme.text_muted)
                    .child(message.into()),
            )
            .into_any_element()
    }

    fn render_scp_upload_fallback(
        &self,
        session_id: SessionId,
        placement: SftpBrowserPlacement,
        technical_message: String,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let destination = self
            .session(session_id)
            .and_then(|session| session.terminal.as_ref())
            .and_then(|terminal| terminal.remote_cwd.clone())
            .unwrap_or_else(|| ".".into());
        let mut destination_args = fluent_bundle::FluentArgs::new();
        destination_args.set("path", destination);
        let destination_text = self.tr_with("scp-upload-destination", &destination_args);
        let upload = text_button(
            SharedString::from(format!(
                "scp-upload-fallback-{}-{}",
                placement.element_suffix(),
                session_id.0
            )),
            self.tr("sftp-upload-files"),
            TextButtonTone::Primary,
            true,
            &self.theme,
        )
        .on_click(cx.listener(move |this, _, _, cx| {
            this.choose_sftp_uploads(session_id, placement, cx);
        }));

        div()
            .flex()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .flex_col()
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h(px(0.0))
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .px_4()
                    .text_center()
                    .child(self.render_sidebar_icon(IconName::Upload, 22.0))
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .child(self.tr("scp-upload-fallback")),
                    )
                    .child(
                        div()
                            .max_w(px(420.0))
                            .text_sm()
                            .text_color(self.theme.text_muted)
                            .child(self.tr("scp-upload-fallback-detail")),
                    )
                    .child(
                        div()
                            .max_w(px(420.0))
                            .text_xs()
                            .font_family(UI_MONOSPACE_FONT_FAMILY)
                            .text_color(self.theme.text_faint)
                            .child(destination_text),
                    )
                    .child(upload)
                    .child(
                        div()
                            .max_w(px(420.0))
                            .text_xs()
                            .text_color(self.theme.text_faint)
                            .child(technical_message),
                    ),
            )
            .child(self.render_sftp_transfer_queue(session_id, placement, cx))
            .into_any_element()
    }

    pub(super) fn render_sftp_breadcrumbs(
        &self,
        session_id: SessionId,
        placement: SftpBrowserPlacement,
        path: &str,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let hover = self.theme.control_hover_bg;
        let pressed = self.theme.control_pressed_bg;
        let scroll_handle = self
            .session(session_id)
            .map(|session| {
                session
                    .sftp_browser(placement)
                    .breadcrumb_scroll_handle
                    .clone()
            })
            .unwrap_or_default();
        let mut breadcrumbs = div()
            .id(SharedString::from(format!(
                "sftp-breadcrumbs-{}",
                placement.element_suffix()
            )))
            .flex()
            .flex_1()
            .w_full()
            .min_w(px(0.0))
            .items_center()
            .overflow_x_scroll()
            .track_scroll(&scroll_handle)
            .font_family(UI_MONOSPACE_FONT_FAMILY)
            .text_sm();
        for (index, (label, target)) in remote_breadcrumbs(path).into_iter().enumerate() {
            if index > 0 {
                breadcrumbs = breadcrumbs.child(
                    div()
                        .flex_none()
                        .px_1()
                        .text_color(self.theme.text_faint)
                        .child("/"),
                );
            }
            breadcrumbs = breadcrumbs.child(
                div()
                    .id(SharedString::from(format!(
                        "sftp-breadcrumb-{}-{index}",
                        placement.element_suffix()
                    )))
                    .flex_none()
                    .max_w(px(120.0))
                    .truncate()
                    .px_1()
                    .py(px(2.0))
                    .rounded_md()
                    .cursor_pointer()
                    .hover(move |this| this.bg(hover))
                    .active(move |this| this.bg(pressed))
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.request_sftp_directory(session_id, placement, target.clone(), cx);
                    })),
            );
        }
        breadcrumbs
    }

    pub(super) fn render_sftp_entry_rows(
        &self,
        session_id: SessionId,
        placement: SftpBrowserPlacement,
        range: Range<usize>,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let tree = placement == SftpBrowserPlacement::Sidebar;
        let Some((tree_rows, selected_paths, connected, loading)) =
            self.session(session_id).map(|session| {
                let browser = session.sftp_browser(placement);
                let tree_rows = browser
                    .visible_rows(tree)
                    .get(range)
                    .map_or_else(Vec::new, <[SftpTreeRow]>::to_vec);
                (
                    tree_rows,
                    browser.selected_paths.clone(),
                    session.connection_state == SessionState::Connected,
                    browser.loading,
                )
            })
        else {
            return Vec::new();
        };
        let list_hover = self.theme.list_hover_bg;
        let pressed = self.theme.control_pressed_bg;
        let selected_background = self.theme.list_selected_bg;
        let element_suffix = placement.element_suffix();
        let mut rows = Vec::with_capacity(tree_rows.len());

        for tree_row in tree_rows {
            let entry = tree_row.entry;
            let is_directory = entry.kind == RemoteFileKind::Directory;
            let is_file = entry.kind == RemoteFileKind::File;
            let entry_path = entry.path.clone();
            let context_entry_path = entry_path.clone();
            let is_selected = selected_paths.contains(&entry_path);
            let is_expanded = self.session(session_id).is_some_and(|session| {
                session
                    .sftp_browser(placement)
                    .expanded_paths
                    .contains(&entry_path)
            });
            let icon_name = if is_directory {
                IconName::Folder
            } else {
                IconName::File
            };
            let size = if is_directory {
                "-".into()
            } else {
                entry
                    .size
                    .map(format_remote_size)
                    .unwrap_or_else(|| "-".into())
            };
            let disclosure = if tree && is_directory {
                let disclosure_path = entry_path.clone();
                div()
                    .id(SharedString::from(format!(
                        "sftp-disclosure-{element_suffix}-{}",
                        entry.path
                    )))
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(px(20.0))
                    .rounded_md()
                    .cursor_pointer()
                    .hover(move |this| this.bg(list_hover))
                    .child(icon(
                        if is_expanded {
                            IconName::Collapse
                        } else {
                            IconName::Expand
                        },
                        self.theme,
                        IconTone::Default,
                        13.0,
                    ))
                    .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                        if !event.standard_click() || event.click_count() != 1 {
                            return;
                        }
                        cx.stop_propagation();
                        this.toggle_remote_tree_directory(
                            session_id,
                            placement,
                            disclosure_path.clone(),
                            cx,
                        );
                    }))
                    .into_any_element()
            } else if tree {
                div().flex_none().size(px(20.0)).into_any_element()
            } else {
                div().flex_none().w(px(0.0)).into_any_element()
            };
            let row_hover = if is_selected {
                selected_background
            } else {
                list_hover
            };
            let mut row = div()
                .id(SharedString::from(format!(
                    "sftp-entry-{element_suffix}-{}",
                    entry.path
                )))
                .flex()
                .flex_none()
                .w_full()
                .min_w(px(0.0))
                .items_center()
                .gap_2()
                .h(px(36.0))
                .pl(px(8.0 + tree_row.depth as f32 * 16.0))
                .pr_3()
                .border_b_1()
                .border_color(self.theme.border)
                .bg(if is_selected {
                    selected_background
                } else {
                    self.theme.transparent
                })
                .child(disclosure)
                .child(self.render_sidebar_icon(icon_name, 16.0))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .truncate()
                        .text_sm()
                        .child(entry.name),
                )
                .child(
                    div()
                        .flex_none()
                        .w(px(88.0))
                        .text_right()
                        .text_sm()
                        .text_color(self.theme.text_muted)
                        .child(size),
                );
            if is_directory || is_file {
                row = row
                    .cursor_pointer()
                    .hover(move |this| this.bg(row_hover))
                    .active(move |this| this.bg(pressed))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            if let Some(session) = this.session_mut(session_id) {
                                session
                                    .sftp_browser_mut(placement)
                                    .select_for_context_menu(&context_entry_path);
                            }
                            this.sftp_context_menu = Some(SftpContextMenu {
                                session_id,
                                placement,
                                position: event.position,
                            });
                            cx.notify();
                        }),
                    )
                    .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                        if !event.standard_click() {
                            return;
                        }
                        if let Some(session) = this.session_mut(session_id) {
                            session.sftp_browser_mut(placement).select_path(
                                &entry_path,
                                event.modifiers(),
                                tree,
                            );
                        }
                        if event.click_count() >= 2 && connected && !loading {
                            if is_directory {
                                if tree {
                                    this.request_sftp_directory(
                                        session_id,
                                        placement,
                                        entry_path.clone(),
                                        cx,
                                    );
                                } else {
                                    this.open_remote_directory(placement, entry_path.clone(), cx);
                                }
                            } else {
                                this.open_remote_file(entry_path.clone(), true, cx);
                            }
                        } else {
                            cx.notify();
                        }
                    }));
            }
            rows.push(row.into_any_element());
        }

        rows
    }

    pub(super) fn render_sftp_transfer_queue(
        &self,
        session_id: SessionId,
        placement: SftpBrowserPlacement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (tasks, batch_progress) = self
            .session(session_id)
            .map(|session| {
                let direction = session.transfers.latest_batch_direction();
                (
                    session.transfers.tasks.clone(),
                    direction.and_then(|direction| {
                        session
                            .transfers
                            .latest_batch_progress(direction)
                            .map(|progress| (direction, progress))
                    }),
                )
            })
            .unwrap_or_default();
        if tasks.is_empty() {
            return div().into_any_element();
        }
        let has_finished = tasks.iter().any(|task| task.state.is_finished());
        let element_suffix = placement.element_suffix();

        let mut clear_button = self.render_icon_button(
            SharedString::from(format!(
                "clear-sftp-transfers-{element_suffix}-{}",
                session_id.0
            )),
            IconName::Delete,
            self.tr("sftp-clear-finished"),
            IconTone::Default,
            has_finished,
        );
        if has_finished {
            clear_button = clear_button.on_click(cx.listener(move |this, _, _, cx| {
                this.clear_finished_sftp_transfers(session_id, cx);
            }));
        }

        let mut queue = div()
            .flex()
            .flex_none()
            .flex_col()
            .max_h(px(220.0))
            .border_t_1()
            .border_color(self.theme.border)
            .bg(self.theme.surface_bg)
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .h(px(36.0))
                    .px_3()
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .child(self.tr("sftp-transfers")),
                    )
                    .child(clear_button),
            );

        if let Some((direction, progress)) = batch_progress {
            let percentage = (progress.fraction * 100.0).round() as u32;
            let title_key = match (
                direction,
                progress.settled_count < progress.task_count,
                progress.failed_count == 0,
            ) {
                (SftpTransferDirection::Upload, true, _) => "sftp-uploading-files",
                (SftpTransferDirection::Upload, false, true) => "sftp-uploaded-files",
                (SftpTransferDirection::Upload, false, false) => "sftp-uploaded-errors",
                (SftpTransferDirection::Download, true, _) => "sftp-downloading-files",
                (SftpTransferDirection::Download, false, true) => "sftp-downloaded-files",
                (SftpTransferDirection::Download, false, false) => "sftp-downloaded-errors",
            };
            let mut title_args = fluent_bundle::FluentArgs::new();
            title_args.set(
                "count",
                if progress.settled_count < progress.task_count || progress.failed_count == 0 {
                    progress.task_count
                } else {
                    progress.failed_count
                },
            );
            let title = self.tr_with(title_key, &title_args);
            let status = progress.total.map_or_else(
                || {
                    let mut args = fluent_bundle::FluentArgs::new();
                    args.set("settled", progress.settled_count);
                    args.set("total", progress.task_count);
                    args.set("percent", percentage);
                    self.tr_with("sftp-file-progress", &args)
                },
                |total| {
                    let mut args = fluent_bundle::FluentArgs::new();
                    args.set("transferred", format_remote_size(progress.transferred));
                    args.set("total", format_remote_size(total));
                    args.set("percent", percentage);
                    self.tr_with("sftp-byte-progress", &args)
                },
            );
            queue = queue.child(
                div()
                    .flex()
                    .flex_none()
                    .flex_col()
                    .gap_2()
                    .px_3()
                    .pb_3()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .truncate()
                                    .text_sm()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(title),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_xs()
                                    .text_color(self.theme.text_muted)
                                    .child(status),
                            ),
                    )
                    .child(
                        div()
                            .h(px(4.0))
                            .w_full()
                            .overflow_hidden()
                            .rounded_full()
                            .bg(self.theme.control_bg)
                            .child(
                                div()
                                    .h_full()
                                    .w(gpui::relative(progress.fraction))
                                    .rounded_full()
                                    .bg(self.theme.accent),
                            ),
                    ),
            );
        }

        let mut rows = div()
            .id(SharedString::from(format!(
                "sftp-transfer-rows-{element_suffix}-{}",
                session_id.0
            )))
            .flex()
            .flex_col()
            .overflow_y_scroll();
        for task in tasks {
            let transfer_id = task.id;
            let direction = task.direction;
            let state = task.state;
            let progress = task
                .total
                .filter(|total| *total > 0)
                .map(|total| (task.transferred as f32 / total as f32).clamp(0.0, 1.0))
                .unwrap_or(0.0);
            let status_color = match state {
                SftpTransferState::Completed => self.theme.status_ok,
                SftpTransferState::Failed => self.theme.error_text,
                SftpTransferState::Conflict => self.theme.status_warn,
                SftpTransferState::Queued
                | SftpTransferState::Running
                | SftpTransferState::Cancelling
                | SftpTransferState::Cancelled => self.theme.text_muted,
            };
            let mut controls = div().flex().flex_none().items_center().gap_1();
            match state {
                SftpTransferState::Conflict => {
                    controls = controls
                        .child(
                            text_button(
                                SharedString::from(format!(
                                    "replace-sftp-transfer-{element_suffix}-{transfer_id}"
                                )),
                                self.tr("common-replace"),
                                TextButtonTone::Primary,
                                true,
                                &self.theme,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.replace_sftp_transfer_destination(
                                        session_id,
                                        transfer_id,
                                        cx,
                                    );
                                },
                            )),
                        )
                        .child(
                            text_button(
                                SharedString::from(format!(
                                    "cancel-conflicted-sftp-transfer-{element_suffix}-{transfer_id}"
                                )),
                                self.tr("common-cancel"),
                                TextButtonTone::Secondary,
                                true,
                                &self.theme,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.cancel_sftp_transfer(session_id, transfer_id, cx);
                                },
                            )),
                        );
                }
                SftpTransferState::Queued | SftpTransferState::Running => {
                    controls = controls.child(
                        self.render_icon_button(
                            SharedString::from(format!(
                                "cancel-active-sftp-transfer-{element_suffix}-{transfer_id}"
                            )),
                            IconName::Cancel,
                            self.tr("sftp-cancel-transfer"),
                            IconTone::Default,
                            true,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.cancel_sftp_transfer(session_id, transfer_id, cx);
                        })),
                    );
                }
                SftpTransferState::Cancelling
                | SftpTransferState::Completed
                | SftpTransferState::Failed
                | SftpTransferState::Cancelled => {}
            }

            let row = div()
                .flex()
                .flex_none()
                .items_center()
                .gap_2()
                .px_3()
                .py_2()
                .border_t_1()
                .border_color(self.theme.border)
                .child(self.render_sidebar_icon(
                    match direction {
                        SftpTransferDirection::Upload => IconName::Upload,
                        SftpTransferDirection::Download => IconName::Download,
                    },
                    16.0,
                ))
                .child(
                    div()
                        .flex()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .truncate()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .child(task.display_name()),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_xs()
                                .text_color(status_color)
                                .child(task.status_text(&self.localizer)),
                        )
                        .when(
                            matches!(
                                state,
                                SftpTransferState::Running | SftpTransferState::Cancelling
                            ),
                            |this| {
                                this.child(
                                    div()
                                        .h(px(3.0))
                                        .w_full()
                                        .overflow_hidden()
                                        .rounded_full()
                                        .bg(self.theme.control_bg)
                                        .child(
                                            div()
                                                .h_full()
                                                .w(gpui::relative(progress))
                                                .rounded_full()
                                                .bg(self.theme.accent),
                                        ),
                                )
                            },
                        ),
                )
                .child(controls);
            rows = rows.child(row);
        }

        queue = queue.child(rows);
        queue.into_any_element()
    }

    pub(super) fn render_sftp_file(
        &self,
        session_id: SessionId,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(session) = self.session(session_id) else {
            return div().into_any_element();
        };
        let Some(file) = session.sftp.file.as_ref() else {
            return div().into_any_element();
        };
        let path = file.path.clone();
        let editor = file.editor.clone();
        let loading = file.loading;
        let saving = file.saving;
        let editable = file.editable;
        let error = file.error.clone();
        let binary = !loading && error.is_none() && file.text_format.is_none();
        let dirty = file.is_dirty(cx);
        let connected = session.connection_state == SessionState::Connected;
        let size = file.original_contents.len() as u64;

        let mut back_button = self.render_icon_button(
            "sftp_close_file",
            IconName::ArrowLeft,
            self.tr("sftp-back-directory"),
            IconTone::Default,
            !saving,
        );
        if !saving {
            back_button = back_button.on_click(cx.listener(move |this, _, _, cx| {
                this.close_remote_file(session_id, cx);
            }));
        }

        let mut revert_button = text_button(
            "sftp_revert_file",
            self.tr("common-revert"),
            TextButtonTone::Secondary,
            dirty && !saving,
            &self.theme,
        );
        if dirty && !saving {
            revert_button = revert_button.on_click(cx.listener(move |this, _, _, cx| {
                this.revert_remote_file(session_id, cx);
            }));
        }

        let can_save = editable && dirty && !saving && connected;
        let mut save_button = text_button(
            "sftp_save_file",
            if saving {
                self.tr("common-saving")
            } else {
                self.tr("common-save")
            },
            TextButtonTone::Primary,
            can_save,
            &self.theme,
        );
        if can_save {
            save_button = save_button.on_click(cx.listener(move |this, _, _, cx| {
                this.save_remote_file(session_id, cx);
            }));
        }

        let mut content = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0));
        if loading {
            content = content
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(self.theme.text_muted)
                .child(self.tr("sftp-loading-file"));
        } else if binary {
            content = content
                .items_center()
                .justify_center()
                .gap_2()
                .text_sm()
                .text_color(self.theme.text_muted)
                .child(self.render_sidebar_icon(IconName::File, 20.0))
                .child(self.tr("sftp-binary-read-only"))
                .child(format_remote_size(size));
        } else if let Some(editor) = editor {
            content = content.child(editor);
        }

        div()
            .id("sftp_file")
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .mt_4()
            .overflow_hidden()
            .rounded_md()
            .border_1()
            .border_color(self.theme.border)
            .bg(self.theme.panel_bg)
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap_1()
                    .h(px(40.0))
                    .px_2()
                    .border_b_1()
                    .border_color(self.theme.border)
                    .child(back_button)
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .ml_2()
                            .truncate()
                            .font_family(UI_MONOSPACE_FONT_FAMILY)
                            .text_sm()
                            .child(path),
                    )
                    .when(dirty, |this| {
                        this.child(
                            div()
                                .flex_none()
                                .mr_2()
                                .text_sm()
                                .text_color(self.theme.text_muted)
                                .child(self.tr("sftp-modified")),
                        )
                    })
                    .when(!editable, |this| {
                        this.child(
                            div()
                                .flex_none()
                                .mr_2()
                                .text_sm()
                                .text_color(self.theme.text_muted)
                                .child(self.tr("sftp-read-only")),
                        )
                    })
                    .when(editable, |this| {
                        this.child(revert_button).child(save_button)
                    }),
            )
            .when_some(error, |this, error| {
                this.child(
                    div()
                        .flex_none()
                        .px_3()
                        .py_2()
                        .border_b_1()
                        .border_color(self.theme.border)
                        .bg(self.theme.control_bg)
                        .text_sm()
                        .text_color(self.theme.error_text)
                        .child(error),
                )
            })
            .child(content)
            .child(self.render_sftp_transfer_queue(session_id, SftpBrowserPlacement::Center, cx))
            .into_any_element()
    }
}
