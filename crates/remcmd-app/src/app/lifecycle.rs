use super::{Context, PromptButton, PromptLevel, RemCmdApp, SessionId, Timer, Window};
use std::time::{Duration, Instant};

#[derive(Clone, Copy)]
pub(super) enum ExitTarget {
    Window,
    Application,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SaveProgress {
    Waiting,
    Ready,
    Blocked,
}

fn save_progress(states: impl IntoIterator<Item = (bool, bool)>) -> SaveProgress {
    let mut dirty = false;
    let mut saving = false;
    for (is_dirty, is_saving) in states {
        dirty |= is_dirty;
        saving |= is_saving;
    }
    if saving {
        SaveProgress::Waiting
    } else if dirty {
        SaveProgress::Blocked
    } else {
        SaveProgress::Ready
    }
}

impl RemCmdApp {
    fn exit_file_states(&self, cx: &Context<Self>) -> Vec<(bool, bool)> {
        self.sessions
            .iter()
            .filter_map(|session| session.sftp.file.as_ref())
            .map(|file| (file.is_dirty(cx), file.saving))
            .collect()
    }

    fn has_pending_transfers(&self) -> bool {
        self.sessions.iter().any(|session| {
            session
                .transfers
                .tasks
                .iter()
                .any(|task| !task.state.is_finished())
        })
    }

    pub(super) fn request_exit(
        &mut self,
        target: ExitTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.exit_task.is_some() {
            return;
        }
        let needs_save = save_progress(self.exit_file_states(cx)) != SaveProgress::Ready;
        if !needs_save && !self.has_pending_transfers() {
            self.finish_exit(target, window, cx);
            return;
        }
        let answer = needs_save.then(|| {
            window.prompt(
                PromptLevel::Warning,
                &self.tr("exit-unsaved-title"),
                Some(&self.tr("exit-unsaved-detail")),
                &[
                    PromptButton::new(self.tr("exit-save-all")),
                    PromptButton::new(self.tr("exit-discard")),
                    PromptButton::cancel(self.tr("common-cancel")),
                ],
                cx,
            )
        });
        self.exit_task = Some(cx.spawn_in(window, async move |this, cx| {
            let mut discard = false;
            if let Some(answer) = answer {
                match answer.await {
                    Ok(0) => {
                        let _ =
                            this.update_in(cx, |this, _, cx| {
                                let sessions: Vec<SessionId> =
                                    this.sessions
                                        .iter()
                                        .filter(|session| {
                                            session.sftp.file.as_ref().is_some_and(|file| {
                                                file.is_dirty(cx) && !file.saving
                                            })
                                        })
                                        .map(|session| session.id)
                                        .collect();
                                for session in sessions {
                                    this.save_remote_file(session, cx);
                                }
                            });
                        let started = Instant::now();
                        loop {
                            let progress = this.update_in(cx, |this, _, cx| {
                                save_progress(this.exit_file_states(cx))
                            });
                            match progress {
                                Ok(SaveProgress::Ready) => break,
                                Ok(SaveProgress::Waiting)
                                    if started.elapsed() < Duration::from_secs(60) =>
                                {
                                    Timer::after(Duration::from_millis(100)).await;
                                }
                                Ok(_) => {
                                    let prompt = this.update_in(cx, |this, window, cx| {
                                        this.exit_task = None;
                                        window.prompt(
                                            PromptLevel::Warning,
                                            &this.tr("exit-save-failed"),
                                            Some(&this.tr("exit-save-failed-detail")),
                                            &[PromptButton::new(this.tr("common-ok"))],
                                            cx,
                                        )
                                    });
                                    if let Ok(answer) = prompt {
                                        let _ = answer.await;
                                    }
                                    return;
                                }
                                Err(_) => return,
                            }
                        }
                    }
                    Ok(1) => discard = true,
                    _ => {
                        let _ = this.update_in(cx, |this, _, _| this.exit_task = None);
                        return;
                    }
                }
            }
            let confirmation = this.update_in(cx, |this, window, cx| {
                this.has_pending_transfers().then(|| {
                    window.prompt(
                        PromptLevel::Warning,
                        &this.tr("exit-transfers-title"),
                        Some(&this.tr("exit-transfers-detail")),
                        &[
                            PromptButton::cancel(this.tr("common-cancel")),
                            PromptButton::new(this.tr("exit-stop-transfers")),
                        ],
                        cx,
                    )
                })
            });
            if let Ok(Some(answer)) = confirmation
                && answer.await != Ok(1)
            {
                let _ = this.update_in(cx, |this, _, _| this.exit_task = None);
                return;
            }
            let _ = this.update_in(cx, |this, window, cx| {
                this.exit_task = None;
                // Edits may have arrived while a transfer confirmation was open.
                if !discard && save_progress(this.exit_file_states(cx)) != SaveProgress::Ready {
                    this.request_exit(target, window, cx);
                    return;
                }
                this.finish_exit(target, window, cx);
            });
        }));
    }

    fn finish_exit(&mut self, target: ExitTarget, window: &mut Window, cx: &mut Context<Self>) {
        match target {
            ExitTarget::Window => window.remove_window(),
            ExitTarget::Application => {
                #[cfg(target_os = "macos")]
                crate::macos_lifecycle::approve_termination();
                cx.quit();
            }
        }
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn request_application_exit(cx: &mut gpui::App) {
    let handle = cx
        .try_global::<super::RemCmdMainWindow>()
        .map(|main| main.0);
    if let Some(handle) = handle
        && handle
            .update(cx, |this, window, cx| {
                this.request_exit(ExitTarget::Application, window, cx)
            })
            .is_ok()
    {
        return;
    }
    crate::macos_lifecycle::approve_termination();
    cx.quit();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_waits_for_every_pending_save() {
        assert_eq!(
            save_progress([(false, false), (false, true)]),
            SaveProgress::Waiting
        );
        assert_eq!(
            save_progress([(true, false), (true, true)]),
            SaveProgress::Waiting
        );
    }

    #[test]
    fn failed_saves_and_edits_made_during_save_keep_window_open() {
        assert_eq!(save_progress([(true, false)]), SaveProgress::Blocked);
        assert_eq!(
            save_progress([(false, false), (true, false)]),
            SaveProgress::Blocked
        );
    }

    #[test]
    fn exit_is_ready_only_when_all_files_are_saved() {
        assert_eq!(save_progress([]), SaveProgress::Ready);
        assert_eq!(save_progress([(false, false)]), SaveProgress::Ready);
    }
}
