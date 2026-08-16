use super::sftp_state::{
    PendingSftpDownloadTree, RemoteTextFormat, SFTP_ERROR_HINT_DURATION, SftpAvailability,
    SftpBrowserPlacement, SftpCreateKind, SftpCreatePrompt, SftpTransferSpec, SftpTransferTask,
    build_local_upload_plan, build_remote_download_plan, collapse_nested_remote_entries,
    join_remote_relative, remote_join_path, remote_parent_path, remote_relative_path,
    sftp_browser_placement_for_request,
};
use super::{
    ClipboardItem, ConnectionHandle, Context, FileEditor, FileEditorEvent, MAX_REMOTE_FILE_BYTES,
    PathPromptOptions, PromptButton, PromptLevel, RemCmdApp, RemoteDirectoryTree, RemoteFile,
    RemoteFileEntry, RemoteFileKind, SessionId, SessionState, SftpOperation, SftpTransferDirection,
    SshErrorKind, SshRuntime, TerminalTabView, TextField, Timer, Window,
};
use gpui::Focusable;
use gpui::prelude::*;

impl RemCmdApp {
    pub(super) fn ensure_sftp_directory(
        &mut self,
        session_id: SessionId,
        placement: SftpBrowserPlacement,
        cx: &mut Context<Self>,
    ) {
        let Some((path, needs_load)) = self.session(session_id).and_then(|session| {
            if session.connection_state == SessionState::Connected
                && session.sftp_availability != SftpAvailability::Available
            {
                return None;
            }
            let path = session
                .terminal
                .as_ref()
                .and_then(|terminal| terminal.remote_cwd.clone())
                .unwrap_or_else(|| ".".into());
            let browser = session.sftp_browser(placement);
            let editor_closed =
                placement == SftpBrowserPlacement::Sidebar || session.sftp.file.is_none();
            let needs_load = editor_closed && browser.needs_request(&path);
            Some((path, needs_load))
        }) else {
            return;
        };
        if needs_load {
            self.request_sftp_directory(session_id, placement, path, cx);
        }
    }

    pub(super) fn show_sftp_error(
        &mut self,
        session_id: SessionId,
        placement: SftpBrowserPlacement,
        error: String,
        cx: &mut Context<Self>,
    ) {
        let generation = self
            .session_mut(session_id)
            .map(|session| session.sftp_browser_mut(placement).set_error(error));
        if let Some(generation) = generation {
            self.schedule_sftp_error_clear(session_id, placement, generation, cx);
        }
        cx.notify();
    }

    pub(super) fn fail_sftp_request(
        &mut self,
        session_id: SessionId,
        placement: SftpBrowserPlacement,
        request_id: u64,
        error: String,
        cx: &mut Context<Self>,
    ) -> bool {
        let result = self.session_mut(session_id).map(|session| {
            let browser = session.sftp_browser_mut(placement);
            let failed = browser.fail_request(request_id, error);
            (failed, browser.error_generation)
        });
        let Some((failed, generation)) = result else {
            return false;
        };
        if failed {
            self.schedule_sftp_error_clear(session_id, placement, generation, cx);
            cx.notify();
        }
        failed
    }

    pub(super) fn schedule_sftp_error_clear(
        &self,
        session_id: SessionId,
        placement: SftpBrowserPlacement,
        generation: u64,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            Timer::after(SFTP_ERROR_HINT_DURATION).await;
            let _ = this.update(cx, |this, cx| {
                let cleared = this.session_mut(session_id).is_some_and(|session| {
                    session
                        .sftp_browser_mut(placement)
                        .clear_error_if_current(generation)
                });
                if cleared {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn request_sftp_directory(
        &mut self,
        session_id: SessionId,
        placement: SftpBrowserPlacement,
        path: String,
        cx: &mut Context<Self>,
    ) {
        let capability_blocks_request = self.session(session_id).is_some_and(|session| {
            session.connection_state == SessionState::Connected
                && session.sftp_availability != SftpAvailability::Available
        });
        if capability_blocks_request {
            if let Some(session) = self.session_mut(session_id) {
                session.sftp_browser_mut(placement).loading = false;
            }
            cx.notify();
            return;
        }

        let handle = self.session(session_id).and_then(|session| {
            (session.connection_state == SessionState::Connected)
                .then(|| session.connection_handle.clone())
                .flatten()
        });
        let Some(handle) = handle else {
            if let Some(session) = self.session_mut(session_id) {
                session.sftp_browser_mut(placement).loading = false;
            }
            self.show_sftp_error(session_id, placement, self.tr("sftp-connect-browse"), cx);
            return;
        };

        let (request_id, request_path) = {
            let session = self
                .session_mut(session_id)
                .expect("SFTP session should still exist");
            let request_id = session
                .sftp_browser_mut(placement)
                .begin_request(path.clone());
            (request_id, path)
        };

        if let Err(error) = handle.read_directory(request_id, request_path) {
            self.fail_sftp_request(session_id, placement, request_id, error.to_string(), cx);
        }
        cx.notify();
    }

    pub(super) fn refresh_active_sftp_directory(
        &mut self,
        placement: SftpBrowserPlacement,
        cx: &mut Context<Self>,
    ) {
        let Some(session_id) = self.active_session_id else {
            return;
        };
        let path = self
            .session(session_id)
            .map(|session| session.sftp_browser(placement).path.clone())
            .unwrap_or_else(|| ".".into());
        self.request_sftp_directory(session_id, placement, path, cx);
    }

    pub(super) fn refresh_sftp_directory_for_session(
        &mut self,
        session_id: SessionId,
        placement: SftpBrowserPlacement,
        cx: &mut Context<Self>,
    ) {
        let path = self
            .session(session_id)
            .map(|session| session.sftp_browser(placement).path.clone());
        if let Some(path) = path {
            self.request_sftp_directory(session_id, placement, path, cx);
        }
    }

    pub(super) fn toggle_remote_tree_directory(
        &mut self,
        session_id: SessionId,
        placement: SftpBrowserPlacement,
        path: String,
        cx: &mut Context<Self>,
    ) {
        let expanded = self.session(session_id).is_some_and(|session| {
            session
                .sftp_browser(placement)
                .expanded_paths
                .contains(&path)
        });
        if expanded {
            if let Some(session) = self.session_mut(session_id) {
                session
                    .sftp_browser_mut(placement)
                    .expanded_paths
                    .remove(&path);
            }
            cx.notify();
            return;
        }
        self.expand_remote_tree_directory(session_id, placement, path, cx);
    }

    pub(super) fn expand_remote_tree_directory(
        &mut self,
        session_id: SessionId,
        placement: SftpBrowserPlacement,
        path: String,
        cx: &mut Context<Self>,
    ) {
        let cached = self.session(session_id).is_some_and(|session| {
            session
                .sftp_browser(placement)
                .tree_entries
                .contains_key(&path)
        });
        let expanded = self.session(session_id).is_some_and(|session| {
            session
                .sftp_browser(placement)
                .expanded_paths
                .contains(&path)
        });
        if expanded {
            return;
        }
        if cached {
            if let Some(session) = self.session_mut(session_id) {
                session
                    .sftp_browser_mut(placement)
                    .expanded_paths
                    .insert(path);
            }
            cx.notify();
            return;
        }

        let handle = self
            .session(session_id)
            .and_then(|session| session.connection_handle.clone());
        let Some(handle) = handle else {
            return;
        };
        let request_id = self
            .session_mut(session_id)
            .expect("SFTP session should still exist")
            .sftp_browser_mut(placement)
            .begin_tree_request(path.clone());
        if let Err(error) = handle.read_directory(request_id, path) {
            self.fail_sftp_request(session_id, placement, request_id, error.to_string(), cx);
        }
        cx.notify();
    }

    pub(super) fn open_remote_directory(
        &mut self,
        placement: SftpBrowserPlacement,
        path: String,
        cx: &mut Context<Self>,
    ) {
        if let Some(session_id) = self.active_session_id {
            self.request_sftp_directory(session_id, placement, path, cx);
        }
    }

    pub(super) fn open_parent_remote_directory(
        &mut self,
        placement: SftpBrowserPlacement,
        cx: &mut Context<Self>,
    ) {
        let Some((session_id, parent)) = self.active_session_id.and_then(|session_id| {
            self.session(session_id)
                .and_then(|session| remote_parent_path(&session.sftp_browser(placement).path))
                .map(|parent| (session_id, parent))
        }) else {
            return;
        };
        self.request_sftp_directory(session_id, placement, parent, cx);
    }

    pub(super) fn open_remote_file(
        &mut self,
        path: String,
        editable: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(session_id) = self.active_session_id else {
            return;
        };
        let handle = self.session(session_id).and_then(|session| {
            (session.connection_state == SessionState::Connected)
                .then(|| session.connection_handle.clone())
                .flatten()
        });
        let Some(handle) = handle else {
            return;
        };

        let request_id = self
            .session_mut(session_id)
            .expect("SFTP session should still exist")
            .sftp
            .begin_file_request(path.clone(), editable);
        if let Some(tab_id) = self.active_tab_id
            && let Some(tab) = self.tab_mut(tab_id)
        {
            tab.view = TerminalTabView::Files;
        }
        if let Err(error) = handle.read_file(request_id, path)
            && let Some(session) = self.session_mut(session_id)
        {
            session
                .sftp
                .fail_file_request(request_id, SftpOperation::ReadFile, error.to_string());
        }
        cx.notify();
    }

    pub(super) fn complete_remote_file_read(
        &mut self,
        session_id: SessionId,
        request_id: u64,
        file: RemoteFile,
        cx: &mut Context<Self>,
    ) {
        let editable = self
            .session(session_id)
            .and_then(|session| session.sftp.file.as_ref())
            .filter(|state| state.read_request_id == Some(request_id))
            .map(|state| state.editable);
        let Some(editable) = editable else {
            return;
        };

        let decoded = RemoteTextFormat::decode(&file.contents);
        let editor = decoded.as_ref().map(|(_, text)| {
            let editor = cx.new(|cx| {
                if editable {
                    FileEditor::new(cx, text.clone())
                } else {
                    FileEditor::new_read_only(cx, text.clone())
                }
            });
            cx.observe(&editor, |_, _, cx| cx.notify()).detach();
            cx.subscribe(&editor, move |this, _, event, cx| match event {
                FileEditorEvent::SaveRequested => this.save_remote_file(session_id, cx),
            })
            .detach();
            editor
        });

        if let Some(state) = self
            .session_mut(session_id)
            .and_then(|session| session.sftp.file.as_mut())
            .filter(|state| state.read_request_id == Some(request_id))
        {
            state.path = file.path;
            state.original_contents = file.contents;
            state.text_format = decoded.map(|(format, _)| format);
            state.editor = editor;
            state.loading = false;
            state.error = None;
            state.read_request_id = None;
        }
    }

    pub(super) fn save_remote_file(&mut self, session_id: SessionId, cx: &mut Context<Self>) {
        let Some((handle, path, expected_contents, contents)) =
            self.session(session_id).and_then(|session| {
                let handle = (session.connection_state == SessionState::Connected)
                    .then(|| session.connection_handle.clone())
                    .flatten()?;
                let file = session.sftp.file.as_ref()?;
                let contents = file.edited_contents(cx)?;
                (!file.loading && !file.saving && contents != file.original_contents).then(|| {
                    (
                        handle,
                        file.path.clone(),
                        file.original_contents.clone(),
                        contents,
                    )
                })
            })
        else {
            return;
        };

        if contents.len() > MAX_REMOTE_FILE_BYTES {
            let mut args = fluent_bundle::FluentArgs::new();
            args.set("size", MAX_REMOTE_FILE_BYTES / 1024 / 1024);
            let message = self.tr_with("sftp-file-limit", &args);
            if let Some(file) = self
                .session_mut(session_id)
                .and_then(|session| session.sftp.file.as_mut())
            {
                file.error = Some(message);
            }
            cx.notify();
            return;
        }

        let Some(request_id) = self
            .session_mut(session_id)
            .and_then(|session| session.sftp.begin_file_save())
        else {
            return;
        };
        if let Err(error) = handle.write_file(request_id, path, expected_contents, contents)
            && let Some(session) = self.session_mut(session_id)
        {
            session
                .sftp
                .fail_file_request(request_id, SftpOperation::WriteFile, error.to_string());
        }
        cx.notify();
    }

    pub(super) fn complete_remote_file_write(
        &mut self,
        session_id: SessionId,
        request_id: u64,
        file: RemoteFile,
    ) {
        let Some(state) = self
            .session_mut(session_id)
            .and_then(|session| session.sftp.file.as_mut())
            .filter(|state| state.write_request_id == Some(request_id))
        else {
            return;
        };
        state.path = file.path;
        state.original_contents = file.contents;
        state.saving = false;
        state.error = None;
        state.write_request_id = None;
    }

    pub(super) fn revert_remote_file(&mut self, session_id: SessionId, cx: &mut Context<Self>) {
        let replacement = self
            .session(session_id)
            .and_then(|session| session.sftp.file.as_ref())
            .and_then(|file| {
                Some((
                    file.editor.clone()?,
                    RemoteTextFormat::decode(&file.original_contents)?.1,
                ))
            });
        if let Some((editor, text)) = replacement {
            editor.update(cx, |editor, cx| editor.replace_all(text, cx));
        }
        if let Some(file) = self
            .session_mut(session_id)
            .and_then(|session| session.sftp.file.as_mut())
        {
            file.error = None;
        }
        cx.notify();
    }

    pub(super) fn close_remote_file(&mut self, session_id: SessionId, cx: &mut Context<Self>) {
        let dirty = self
            .session(session_id)
            .and_then(|session| session.sftp.file.as_ref())
            .is_some_and(|file| file.is_dirty(cx));
        if dirty {
            let message = self.tr("sftp-save-before-close");
            if let Some(file) = self
                .session_mut(session_id)
                .and_then(|session| session.sftp.file.as_mut())
            {
                file.error = Some(message);
            }
        } else if let Some(session) = self.session_mut(session_id) {
            session.sftp.file = None;
        }
        cx.notify();
    }

    pub(super) fn choose_sftp_uploads(
        &mut self,
        session_id: SessionId,
        placement: SftpBrowserPlacement,
        cx: &mut Context<Self>,
    ) {
        let Some(remote_directory) =
            self.session(session_id)
                .map(|session| match &session.sftp_availability {
                    SftpAvailability::ScpOnly(_) => session
                        .terminal
                        .as_ref()
                        .and_then(|terminal| terminal.remote_cwd.clone())
                        .unwrap_or_else(|| ".".into()),
                    _ => session.sftp_browser(placement).path.clone(),
                })
        else {
            return;
        };
        let selected_paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: true,
            multiple: true,
            prompt: Some(self.tr("sftp-upload").into()),
        });
        let runtime = cx.global::<SshRuntime>().handle();

        cx.spawn(async move |this, cx| match selected_paths.await {
            Ok(Ok(Some(paths))) => {
                let plan = runtime
                    .spawn_blocking(move || build_local_upload_plan(&paths, &remote_directory))
                    .await;
                let _ = this.update(cx, |this, cx| {
                    let Ok(Ok(plan)) = plan else {
                        let message = match plan {
                            Ok(Err(error)) => error.to_string(),
                            Err(error) => error.to_string(),
                            Ok(Ok(_)) => unreachable!(),
                        };
                        this.show_sftp_error(
                            session_id,
                            placement,
                            format!("{}: {message}", this.tr("sftp-prepare-upload-failed")),
                            cx,
                        );
                        return;
                    };

                    if !plan.directories.is_empty() {
                        let request_id = this
                            .session_mut(session_id)
                            .map(|session| session.sftp_browser_mut(placement).next_request_id());
                        let create_result = request_id
                            .zip(
                                this.session(session_id)
                                    .and_then(|session| session.connection_handle.clone()),
                            )
                            .map(|(request_id, handle)| {
                                handle.create_directories(request_id, plan.directories)
                            });
                        if !matches!(create_result, Some(Ok(()))) {
                            this.show_sftp_error(
                                session_id,
                                placement,
                                this.tr("sftp-queue-directory-failed"),
                                cx,
                            );
                            return;
                        }
                    }

                    let Some(batch_id) = this
                        .session_mut(session_id)
                        .map(|session| session.transfers.begin_batch())
                    else {
                        return;
                    };
                    for (local_path, remote_path, size) in plan.files {
                        this.enqueue_sftp_transfer(
                            session_id,
                            SftpTransferSpec {
                                batch_id,
                                direction: SftpTransferDirection::Upload,
                                local_path,
                                remote_path,
                                overwrite: false,
                                expected_total: Some(size),
                            },
                            cx,
                        );
                    }
                });
            }
            Ok(Ok(None)) | Err(_) => {}
            Ok(Err(error)) => {
                let _ = this.update(cx, |this, cx| {
                    this.show_sftp_error(
                        session_id,
                        placement,
                        format!("{}: {error}", this.tr("sftp-open-upload-failed")),
                        cx,
                    );
                });
            }
        })
        .detach();
    }

    pub(super) fn choose_sftp_downloads(
        &mut self,
        session_id: SessionId,
        placement: SftpBrowserPlacement,
        entries: Vec<RemoteFileEntry>,
        cx: &mut Context<Self>,
    ) {
        if entries.is_empty() {
            return;
        }
        let entries = collapse_nested_remote_entries(entries);
        let browser_root = self
            .session(session_id)
            .map(|session| session.sftp_browser(placement).path.clone())
            .unwrap_or_else(|| ".".into());
        let selected_paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(self.tr("sftp-download").into()),
        });

        cx.spawn(async move |this, cx| match selected_paths.await {
            Ok(Ok(Some(paths))) => {
                let Some(destination) = paths.into_iter().next() else {
                    return;
                };
                let _ = this.update(cx, |this, cx| {
                    let handle = this
                        .session(session_id)
                        .and_then(|session| session.connection_handle.clone());
                    let Some(handle) = handle else {
                        return;
                    };
                    let Some(batch_id) = this
                        .session_mut(session_id)
                        .map(|session| session.transfers.begin_batch())
                    else {
                        return;
                    };

                    for entry in entries {
                        let local_path = remote_relative_path(&browser_root, &entry.path)
                            .map(|relative| join_remote_relative(&destination, relative))
                            .unwrap_or_else(|| destination.join(&entry.name));
                        if entry.kind == RemoteFileKind::Directory {
                            let request_id = this
                                .session_mut(session_id)
                                .expect("SFTP session should still exist")
                                .sftp_browser_mut(placement)
                                .begin_download_tree(local_path, batch_id);
                            if let Err(error) =
                                handle.read_directory_tree(request_id, entry.path.clone())
                            {
                                this.fail_sftp_request(
                                    session_id,
                                    placement,
                                    request_id,
                                    error.to_string(),
                                    cx,
                                );
                            }
                        } else if entry.kind == RemoteFileKind::File {
                            this.enqueue_sftp_transfer(
                                session_id,
                                SftpTransferSpec {
                                    batch_id,
                                    direction: SftpTransferDirection::Download,
                                    local_path,
                                    remote_path: entry.path,
                                    overwrite: false,
                                    expected_total: entry.size,
                                },
                                cx,
                            );
                        }
                    }
                });
            }
            Ok(Ok(None)) | Err(_) => {}
            Ok(Err(error)) => {
                let _ = this.update(cx, |this, cx| {
                    this.show_sftp_error(
                        session_id,
                        placement,
                        format!("{}: {error}", this.tr("sftp-open-download-failed")),
                        cx,
                    );
                });
            }
        })
        .detach();
    }

    pub(super) fn complete_directory_tree_download(
        &mut self,
        session_id: SessionId,
        request_id: u64,
        tree: RemoteDirectoryTree,
        cx: &mut Context<Self>,
    ) {
        let placement = sftp_browser_placement_for_request(request_id);
        let pending = self.session_mut(session_id).and_then(|session| {
            session
                .sftp_browser_mut(placement)
                .take_download_tree(request_id)
        });
        let Some(pending) = pending else {
            return;
        };
        let PendingSftpDownloadTree {
            destination,
            batch_id,
        } = pending;
        let runtime = cx.global::<SshRuntime>().handle();

        cx.spawn(async move |this, cx| {
            let plan = runtime
                .spawn_blocking(move || build_remote_download_plan(tree, destination))
                .await;
            let _ = this.update(cx, |this, cx| match plan {
                Ok(Ok(plan)) => {
                    for (local_path, remote_path, size) in plan {
                        this.enqueue_sftp_transfer(
                            session_id,
                            SftpTransferSpec {
                                batch_id,
                                direction: SftpTransferDirection::Download,
                                local_path,
                                remote_path,
                                overwrite: false,
                                expected_total: size,
                            },
                            cx,
                        );
                    }
                }
                Ok(Err(error)) => {
                    this.show_sftp_error(
                        session_id,
                        placement,
                        format!("{}: {error}", this.tr("sftp-prepare-download-failed")),
                        cx,
                    );
                }
                Err(error) => {
                    this.show_sftp_error(
                        session_id,
                        placement,
                        format!("{}: {error}", this.tr("sftp-download-task-failed")),
                        cx,
                    );
                }
            });
        })
        .detach();
    }

    pub(super) fn selected_sftp_entries(
        &self,
        session_id: SessionId,
        placement: SftpBrowserPlacement,
    ) -> Vec<RemoteFileEntry> {
        self.session(session_id)
            .map(|session| session.sftp_browser(placement).selected_entries())
            .unwrap_or_default()
    }

    pub(super) fn download_selected_sftp_entries(
        &mut self,
        session_id: SessionId,
        placement: SftpBrowserPlacement,
        cx: &mut Context<Self>,
    ) {
        let entries = self.selected_sftp_entries(session_id, placement);
        self.sftp_context_menu = None;
        self.choose_sftp_downloads(session_id, placement, entries, cx);
    }

    pub(super) fn open_selected_sftp_file(
        &mut self,
        session_id: SessionId,
        placement: SftpBrowserPlacement,
        editable: bool,
        cx: &mut Context<Self>,
    ) {
        let entry = self
            .selected_sftp_entries(session_id, placement)
            .into_iter()
            .next()
            .filter(|entry| entry.kind == RemoteFileKind::File);
        self.sftp_context_menu = None;
        if let Some(entry) = entry {
            self.open_remote_file(entry.path, editable, cx);
        }
    }

    pub(super) fn copy_selected_sftp_paths(
        &mut self,
        session_id: SessionId,
        placement: SftpBrowserPlacement,
        cx: &mut Context<Self>,
    ) {
        let paths = self
            .selected_sftp_entries(session_id, placement)
            .into_iter()
            .map(|entry| entry.path)
            .collect::<Vec<_>>();
        if !paths.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(paths.join("\n")));
        }
        self.sftp_context_menu = None;
        cx.notify();
    }

    pub(super) fn delete_selected_sftp_entries(
        &mut self,
        session_id: SessionId,
        placement: SftpBrowserPlacement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let entries =
            collapse_nested_remote_entries(self.selected_sftp_entries(session_id, placement));
        if entries.is_empty() {
            return;
        }
        let paths = entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect::<Vec<_>>();
        let message = if paths.len() == 1 {
            let mut args = fluent_bundle::FluentArgs::new();
            args.set("name", entries[0].name.clone());
            self.tr_with("sftp-delete-one", &args)
        } else {
            let mut args = fluent_bundle::FluentArgs::new();
            args.set("count", paths.len());
            self.tr_with("sftp-delete-many", &args)
        };
        self.sftp_context_menu = None;
        let answer = window.prompt(
            PromptLevel::Critical,
            &message,
            Some(&self.tr("sftp-delete-recursive")),
            &[
                PromptButton::new(self.tr("common-delete")),
                PromptButton::cancel(self.tr("common-cancel")),
            ],
            cx,
        );

        cx.spawn_in(window, async move |this, cx| {
            if answer.await != Ok(0) {
                return;
            }
            let _ = this.update(cx, |this, cx| {
                let handle = this
                    .session(session_id)
                    .and_then(|session| session.connection_handle.clone());
                let Some(handle) = handle else {
                    return;
                };
                let request_id = this
                    .session_mut(session_id)
                    .expect("SFTP session should still exist")
                    .sftp_browser_mut(placement)
                    .next_request_id();
                if let Err(error) = handle.delete_paths(request_id, paths) {
                    this.show_sftp_error(session_id, placement, error.to_string(), cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn open_sftp_create_prompt(
        &mut self,
        session_id: SessionId,
        placement: SftpBrowserPlacement,
        kind: SftpCreateKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let placeholder = match kind {
            SftpCreateKind::File => self.tr("sftp-file-name"),
            SftpCreateKind::Directory => self.tr("sftp-folder-name"),
        };
        let input = cx.new(|cx| TextField::new(cx, "", placeholder));
        cx.observe(&input, |_, _, cx| cx.notify()).detach();
        input.focus_handle(cx).focus(window);
        self.sftp_context_menu = None;
        self.sftp_create_prompt = Some(SftpCreatePrompt {
            session_id,
            placement,
            kind,
            input,
            error: None,
        });
        cx.notify();
    }

    pub(super) fn submit_sftp_create(&mut self, cx: &mut Context<Self>) {
        let Some(prompt) = self.sftp_create_prompt.as_ref() else {
            return;
        };
        let name = prompt.input.read(cx).text().trim().to_owned();
        if name.is_empty()
            || name == "."
            || name == ".."
            || name.contains('/')
            || name.contains('\\')
        {
            let message = self.tr("sftp-valid-name");
            if let Some(prompt) = self.sftp_create_prompt.as_mut() {
                prompt.error = Some(message);
            }
            cx.notify();
            return;
        }

        let session_id = prompt.session_id;
        let placement = prompt.placement;
        let kind = prompt.kind;
        let directory = self
            .session(session_id)
            .map(|session| session.sftp_browser(placement).path.clone())
            .unwrap_or_else(|| ".".into());
        let path = remote_join_path(&directory, &name);
        let handle = self
            .session(session_id)
            .and_then(|session| session.connection_handle.clone());
        let Some(handle) = handle else {
            return;
        };
        let request_id = self
            .session_mut(session_id)
            .expect("SFTP session should still exist")
            .sftp_browser_mut(placement)
            .next_request_id();
        let result = match kind {
            SftpCreateKind::File => handle.create_file(request_id, path),
            SftpCreateKind::Directory => handle.create_directories(request_id, vec![path]),
        };
        match result {
            Ok(()) => {
                self.sftp_create_prompt = None;
            }
            Err(error) => {
                if let Some(prompt) = self.sftp_create_prompt.as_mut() {
                    prompt.error = Some(error.to_string());
                }
            }
        }
        cx.notify();
    }

    pub(super) fn enqueue_sftp_transfer(
        &mut self,
        session_id: SessionId,
        transfer: SftpTransferSpec,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.session_mut(session_id) else {
            return;
        };
        session.transfers.enqueue_in_batch(
            transfer.batch_id,
            transfer.direction,
            transfer.local_path,
            transfer.remote_path,
            transfer.overwrite,
            transfer.expected_total,
        );
        self.start_queued_sftp_transfers(cx);
        cx.notify();
    }

    pub(super) fn active_sftp_transfer_count(&self) -> usize {
        self.sessions
            .iter()
            .map(|session| session.transfers.active_count())
            .sum()
    }

    pub(super) fn take_next_queued_sftp_transfer(
        &mut self,
    ) -> Option<(SessionId, Option<ConnectionHandle>, SftpTransferTask)> {
        if self.sessions.is_empty() {
            self.next_transfer_session_cursor = 0;
            return None;
        }

        let session_count = self.sessions.len();
        for offset in 0..session_count {
            let index = (self.next_transfer_session_cursor + offset) % session_count;
            let session = &mut self.sessions[index];
            let Some(task) = session.transfers.start_next() else {
                continue;
            };
            let handle = (session.connection_state == SessionState::Connected)
                .then(|| session.connection_handle.clone())
                .flatten();
            self.next_transfer_session_cursor = (index + 1) % session_count;
            return Some((session.id, handle, task));
        }
        None
    }

    pub(super) fn start_queued_sftp_transfers(&mut self, cx: &mut Context<Self>) {
        let connect_before_transfer = self.tr("sftp-connect-transfer");
        loop {
            if self.active_sftp_transfer_count()
                >= usize::from(self.transfer_settings.max_parallel_transfers)
            {
                break;
            }
            let Some((session_id, handle, task)) = self.take_next_queued_sftp_transfer() else {
                break;
            };
            let transfer_id = task.id;

            let result = match handle {
                Some(handle) => match task.direction {
                    SftpTransferDirection::Upload => handle.upload_file(
                        task.id,
                        task.local_path,
                        task.remote_path,
                        task.overwrite,
                    ),
                    SftpTransferDirection::Download => handle.download_file(
                        task.id,
                        task.remote_path,
                        task.local_path,
                        task.overwrite,
                    ),
                },
                None => Err(remcmd_ssh::SshError::new(
                    SshErrorKind::InvalidState,
                    connect_before_transfer.clone(),
                )),
            };

            match result {
                Ok(()) => {}
                Err(error) => {
                    if let Some(session) = self.session_mut(session_id) {
                        session
                            .transfers
                            .mark_failed(transfer_id, error.to_string());
                    }
                }
            }
        }
        cx.notify();
    }

    pub(super) fn cancel_sftp_transfer(
        &mut self,
        session_id: SessionId,
        transfer_id: u64,
        cx: &mut Context<Self>,
    ) {
        let Some((signal_worker, handle)) = self.session_mut(session_id).and_then(|session| {
            let signal_worker = session.transfers.begin_cancel(transfer_id)?;
            Some((signal_worker, session.connection_handle.clone()))
        }) else {
            return;
        };

        if signal_worker {
            let task_missing = self.tr("sftp-task-missing");
            let result = handle
                .ok_or_else(|| remcmd_ssh::SshError::new(SshErrorKind::InvalidState, task_missing))
                .and_then(|handle| handle.cancel_transfer(transfer_id));
            if let Err(error) = result
                && let Some(session) = self.session_mut(session_id)
            {
                session
                    .transfers
                    .mark_failed(transfer_id, error.to_string());
                self.start_queued_sftp_transfers(cx);
            }
        } else {
            self.start_queued_sftp_transfers(cx);
        }
        cx.notify();
    }

    pub(super) fn replace_sftp_transfer_destination(
        &mut self,
        session_id: SessionId,
        transfer_id: u64,
        cx: &mut Context<Self>,
    ) {
        if self
            .session_mut(session_id)
            .is_some_and(|session| session.transfers.retry_with_overwrite(transfer_id))
        {
            self.start_queued_sftp_transfers(cx);
        }
        cx.notify();
    }

    pub(super) fn clear_finished_sftp_transfers(
        &mut self,
        session_id: SessionId,
        cx: &mut Context<Self>,
    ) {
        if let Some(session) = self.session_mut(session_id) {
            session.transfers.clear_finished();
        }
        cx.notify();
    }

    pub(super) fn complete_sftp_transfer(
        &mut self,
        session_id: SessionId,
        transfer_id: u64,
        direction: SftpTransferDirection,
        remote_path: String,
        bytes: u64,
        cx: &mut Context<Self>,
    ) {
        let completed = self
            .session_mut(session_id)
            .is_some_and(|session| session.transfers.mark_completed(transfer_id, bytes));
        if !completed {
            return;
        }

        if direction == SftpTransferDirection::Upload {
            let parent = remote_parent_path(&remote_path).unwrap_or_else(|| ".".into());
            let placements = [SftpBrowserPlacement::Center, SftpBrowserPlacement::Sidebar]
                .into_iter()
                .filter(|placement| {
                    self.session(session_id).is_some_and(|session| {
                        let browser = session.sftp_browser(*placement);
                        browser.loaded && browser.path == parent
                    })
                })
                .collect::<Vec<_>>();
            for placement in placements {
                self.request_sftp_directory(session_id, placement, parent.clone(), cx);
            }
        }

        self.start_queued_sftp_transfers(cx);
    }
}
