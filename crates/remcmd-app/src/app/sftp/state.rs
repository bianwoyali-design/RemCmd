use super::super::{
    App, Entity, FileEditor, Localizer, Pixels, RemoteDirectory, RemoteFileEntry, RemoteFileKind,
    ScrollHandle, SessionId, SftpOperation, SftpTransferDirection, TextField,
    UniformListScrollHandle,
};
use remcmd_ssh::remote_path::{remote_file_name, remote_path_depth, remote_path_is_descendant};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    time::Duration,
};

pub(in crate::app) const SFTP_ERROR_HINT_DURATION: Duration = Duration::from_secs(3);

pub(in crate::app) const SIDEBAR_SFTP_REQUEST_ID_START: u64 = 1 << 63;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum SftpBrowserPlacement {
    Center,
    Sidebar,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::app) enum SftpAvailability {
    #[default]
    Checking,
    Available,
    ScpOnly(String),
    Unavailable(String),
}

impl SftpBrowserPlacement {
    pub(in crate::app) fn element_suffix(self) -> &'static str {
        match self {
            Self::Center => "center",
            Self::Sidebar => "sidebar",
        }
    }
}

pub(in crate::app) struct SftpBrowserState {
    pub(in crate::app) path: String,
    pub(in crate::app) entries: Vec<RemoteFileEntry>,
    pub(in crate::app) file: Option<SftpFileState>,
    pub(in crate::app) loading: bool,
    pub(in crate::app) loaded: bool,
    pub(in crate::app) error: Option<String>,
    pub(in crate::app) next_request_id: u64,
    pub(in crate::app) active_request_id: Option<u64>,
    pub(in crate::app) active_request_path: Option<String>,
    pub(in crate::app) resolved_source_path: Option<String>,
    pub(in crate::app) tree_entries: HashMap<String, Vec<RemoteFileEntry>>,
    pub(in crate::app) expanded_paths: HashSet<String>,
    pub(in crate::app) tree_requests: HashMap<u64, String>,
    pub(in crate::app) pending_download_trees: HashMap<u64, PendingSftpDownloadTree>,
    pub(in crate::app) selected_paths: Vec<String>,
    pub(in crate::app) selection_anchor: Option<String>,
    pub(in crate::app) scroll_handle: UniformListScrollHandle,
    pub(in crate::app) breadcrumb_scroll_handle: ScrollHandle,
    pub(in crate::app) error_generation: u64,
}

#[derive(Clone)]
pub(in crate::app) struct SftpTreeRow {
    pub(in crate::app) entry: RemoteFileEntry,
    pub(in crate::app) depth: usize,
}

pub(in crate::app) struct PendingSftpDownloadTree {
    pub(in crate::app) destination: PathBuf,
    pub(in crate::app) batch_id: u64,
}

impl Default for SftpBrowserState {
    fn default() -> Self {
        Self {
            path: ".".into(),
            entries: Vec::new(),
            file: None,
            loading: false,
            loaded: false,
            error: None,
            next_request_id: 1,
            active_request_id: None,
            active_request_path: None,
            resolved_source_path: None,
            tree_entries: HashMap::new(),
            expanded_paths: HashSet::new(),
            tree_requests: HashMap::new(),
            pending_download_trees: HashMap::new(),
            selected_paths: Vec::new(),
            selection_anchor: None,
            scroll_handle: UniformListScrollHandle::new(),
            breadcrumb_scroll_handle: ScrollHandle::new(),
            error_generation: 0,
        }
    }
}

impl SftpBrowserState {
    pub(in crate::app) fn with_request_id_start(next_request_id: u64) -> Self {
        Self {
            next_request_id,
            ..Self::default()
        }
    }

    pub(in crate::app) fn needs_request(&self, path: &str) -> bool {
        self.active_request_path.as_deref() != Some(path)
            && self.resolved_source_path.as_deref() != Some(path)
    }

    pub(in crate::app) fn begin_request(&mut self, path: String) -> u64 {
        let request_id = self.next_request_id;
        self.next_request_id += 1;
        if !self.loaded || self.path != path {
            self.scroll_handle = UniformListScrollHandle::new();
        }
        self.active_request_id = Some(request_id);
        self.active_request_path = Some(path);
        self.loading = true;
        self.clear_error();
        self.file = None;
        self.tree_entries.clear();
        self.expanded_paths.clear();
        self.tree_requests.clear();
        self.selected_paths.clear();
        self.selection_anchor = None;
        request_id
    }

    pub(in crate::app) fn complete_request(
        &mut self,
        request_id: u64,
        directory: RemoteDirectory,
    ) -> bool {
        if let Some(requested_path) = self.tree_requests.remove(&request_id) {
            self.expanded_paths.remove(&requested_path);
            self.expanded_paths.insert(directory.path.clone());
            self.tree_entries.insert(directory.path, directory.entries);
            self.clear_error();
            return true;
        }
        if self.active_request_id != Some(request_id) {
            return false;
        }

        let breadcrumb_count = remote_breadcrumbs(&directory.path).len();
        self.path = directory.path;
        self.entries = directory.entries;
        self.loading = false;
        self.loaded = true;
        self.clear_error();
        self.active_request_id = None;
        self.resolved_source_path = self.active_request_path.take();
        self.breadcrumb_scroll_handle
            .scroll_to_item(breadcrumb_count.saturating_mul(2).saturating_sub(2));
        true
    }

    pub(in crate::app) fn fail_request(&mut self, request_id: u64, error: String) -> bool {
        if let Some(path) = self.tree_requests.remove(&request_id) {
            self.expanded_paths.remove(&path);
            self.set_error(error);
            return true;
        }
        if self.pending_download_trees.remove(&request_id).is_some() {
            self.set_error(error);
            return true;
        }
        if self.active_request_id != Some(request_id) {
            return false;
        }

        self.loading = false;
        self.set_error(error);
        self.active_request_id = None;
        self.active_request_path = None;
        self.tree_requests.clear();
        self.pending_download_trees.clear();
        true
    }

    pub(in crate::app) fn stop_loading(&mut self) {
        self.loading = false;
        self.active_request_id = None;
        self.active_request_path = None;
        if let Some(file) = self.file.as_mut() {
            file.loading = false;
            file.saving = false;
            file.read_request_id = None;
            file.write_request_id = None;
        }
    }

    pub(in crate::app) fn next_request_id(&mut self) -> u64 {
        let request_id = self.next_request_id;
        self.next_request_id += 1;
        request_id
    }

    pub(in crate::app) fn begin_tree_request(&mut self, path: String) -> u64 {
        let request_id = self.next_request_id();
        self.tree_requests.insert(request_id, path.clone());
        self.expanded_paths.insert(path);
        self.clear_error();
        request_id
    }

    pub(in crate::app) fn begin_download_tree(
        &mut self,
        destination: PathBuf,
        batch_id: u64,
    ) -> u64 {
        let request_id = self.next_request_id();
        self.pending_download_trees.insert(
            request_id,
            PendingSftpDownloadTree {
                destination,
                batch_id,
            },
        );
        request_id
    }

    pub(in crate::app) fn take_download_tree(
        &mut self,
        request_id: u64,
    ) -> Option<PendingSftpDownloadTree> {
        self.pending_download_trees.remove(&request_id)
    }

    pub(in crate::app) fn set_error(&mut self, error: String) -> u64 {
        self.error_generation = self.error_generation.wrapping_add(1);
        self.error = Some(error);
        self.error_generation
    }

    pub(in crate::app) fn clear_error(&mut self) {
        self.error_generation = self.error_generation.wrapping_add(1);
        self.error = None;
    }

    pub(in crate::app) fn clear_error_if_current(&mut self, generation: u64) -> bool {
        if self.error_generation != generation || self.error.is_none() {
            return false;
        }
        self.clear_error();
        true
    }

    pub(in crate::app) fn visible_rows(&self, tree: bool) -> Vec<SftpTreeRow> {
        if !tree {
            return self
                .entries
                .iter()
                .cloned()
                .map(|entry| SftpTreeRow { entry, depth: 0 })
                .collect();
        }

        let mut rows = Vec::new();
        let mut pending = self
            .entries
            .iter()
            .rev()
            .cloned()
            .map(|entry| SftpTreeRow { entry, depth: 0 })
            .collect::<Vec<_>>();
        while let Some(row) = pending.pop() {
            let path = row.entry.path.clone();
            let depth = row.depth;
            rows.push(row);
            if self.expanded_paths.contains(&path)
                && let Some(children) = self.tree_entries.get(&path)
            {
                pending.extend(children.iter().rev().cloned().map(|entry| SftpTreeRow {
                    entry,
                    depth: depth + 1,
                }));
            }
        }
        rows
    }

    pub(in crate::app) fn selected_entries(&self) -> Vec<RemoteFileEntry> {
        self.selected_paths
            .iter()
            .filter_map(|path| self.entry(path).cloned())
            .collect()
    }

    pub(in crate::app) fn entry(&self, path: &str) -> Option<&RemoteFileEntry> {
        self.entries
            .iter()
            .chain(self.tree_entries.values().flatten())
            .find(|entry| entry.path == path)
    }

    pub(in crate::app) fn select_path(
        &mut self,
        path: &str,
        modifiers: gpui::Modifiers,
        tree: bool,
    ) {
        let visible_paths = self
            .visible_rows(tree)
            .into_iter()
            .map(|row| row.entry.path)
            .collect::<Vec<_>>();
        let Some(clicked_index) = visible_paths.iter().position(|candidate| candidate == path)
        else {
            return;
        };

        if modifiers.shift {
            let anchor_index = self
                .selection_anchor
                .as_ref()
                .and_then(|anchor| {
                    visible_paths
                        .iter()
                        .position(|candidate| candidate == anchor)
                })
                .unwrap_or(clicked_index);
            let range = anchor_index.min(clicked_index)..=anchor_index.max(clicked_index);
            if !modifiers.secondary() {
                self.selected_paths.clear();
            }
            for index in range {
                let path = &visible_paths[index];
                if !self.selected_paths.contains(path) {
                    self.selected_paths.push(path.clone());
                }
            }
        } else if modifiers.secondary() {
            if let Some(index) = self
                .selected_paths
                .iter()
                .position(|candidate| candidate == path)
            {
                self.selected_paths.remove(index);
            } else {
                self.selected_paths.push(path.to_owned());
            }
            self.selection_anchor = Some(path.to_owned());
        } else {
            self.selected_paths.clear();
            self.selected_paths.push(path.to_owned());
            self.selection_anchor = Some(path.to_owned());
        }
    }

    pub(in crate::app) fn select_for_context_menu(&mut self, path: &str) {
        if !self.selected_paths.iter().any(|selected| selected == path) {
            self.selected_paths.clear();
            self.selected_paths.push(path.to_owned());
            self.selection_anchor = Some(path.to_owned());
        }
    }

    pub(in crate::app) fn remove_paths(&mut self, paths: &[String]) {
        self.selected_paths
            .retain(|selected| !paths.iter().any(|path| selected == path));
        self.tree_entries.retain(|path, _| {
            !paths
                .iter()
                .any(|deleted| path == deleted || remote_path_is_descendant(deleted, path))
        });
        for entries in self.tree_entries.values_mut() {
            entries.retain(|entry| {
                !paths
                    .iter()
                    .any(|path| entry.path == *path || remote_path_is_descendant(path, &entry.path))
            });
        }
        self.entries.retain(|entry| {
            !paths
                .iter()
                .any(|path| entry.path == *path || remote_path_is_descendant(path, &entry.path))
        });
    }

    pub(in crate::app) fn begin_file_request(&mut self, path: String, editable: bool) -> u64 {
        let request_id = self.next_request_id();
        self.file = Some(SftpFileState {
            path,
            original_contents: Vec::new(),
            editor: None,
            text_format: None,
            loading: true,
            saving: false,
            error: None,
            editable,
            read_request_id: Some(request_id),
            write_request_id: None,
        });
        request_id
    }

    pub(in crate::app) fn begin_file_save(&mut self) -> Option<u64> {
        let request_id = self.next_request_id();
        let file = self.file.as_mut()?;
        file.saving = true;
        file.error = None;
        file.write_request_id = Some(request_id);
        Some(request_id)
    }

    pub(in crate::app) fn fail_file_request(
        &mut self,
        request_id: u64,
        operation: SftpOperation,
        error: String,
    ) {
        let Some(file) = self.file.as_mut() else {
            return;
        };
        match operation {
            SftpOperation::ReadFile if file.read_request_id == Some(request_id) => {
                file.loading = false;
                file.read_request_id = None;
                file.error = Some(error);
            }
            SftpOperation::WriteFile if file.write_request_id == Some(request_id) => {
                file.saving = false;
                file.write_request_id = None;
                file.error = Some(error);
            }
            SftpOperation::ReadDirectory
            | SftpOperation::ReadDirectoryTree
            | SftpOperation::ReadFile
            | SftpOperation::WriteFile
            | SftpOperation::CreateFile
            | SftpOperation::CreateDirectory
            | SftpOperation::DeletePaths
            | SftpOperation::UploadFile
            | SftpOperation::DownloadFile
            | SftpOperation::CancelTransfer => {}
        }
    }

    pub(in crate::app) fn display_path(&self) -> &str {
        self.file
            .as_ref()
            .map(|file| file.path.as_str())
            .unwrap_or(&self.path)
    }
}

pub(in crate::app) struct SftpFileState {
    pub(in crate::app) path: String,
    pub(in crate::app) original_contents: Vec<u8>,
    pub(in crate::app) editor: Option<Entity<FileEditor>>,
    pub(in crate::app) text_format: Option<RemoteTextFormat>,
    pub(in crate::app) loading: bool,
    pub(in crate::app) saving: bool,
    pub(in crate::app) error: Option<String>,
    pub(in crate::app) editable: bool,
    pub(in crate::app) read_request_id: Option<u64>,
    pub(in crate::app) write_request_id: Option<u64>,
}

pub(in crate::app) struct SftpContextMenu {
    pub(in crate::app) session_id: SessionId,
    pub(in crate::app) placement: SftpBrowserPlacement,
    pub(in crate::app) position: gpui::Point<Pixels>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::app) enum SftpCreateKind {
    File,
    Directory,
}

pub(in crate::app) struct SftpCreatePrompt {
    pub(in crate::app) session_id: SessionId,
    pub(in crate::app) placement: SftpBrowserPlacement,
    pub(in crate::app) kind: SftpCreateKind,
    pub(in crate::app) input: Entity<TextField>,
    pub(in crate::app) error: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::app) enum SftpTransferState {
    Queued,
    Running,
    Cancelling,
    Conflict,
    Completed,
    Failed,
    Cancelled,
}

impl SftpTransferState {
    pub(in crate::app) const fn is_active(self) -> bool {
        matches!(self, Self::Running | Self::Cancelling)
    }

    pub(in crate::app) const fn is_finished(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

#[derive(Clone, Debug)]
pub(in crate::app) struct SftpTransferTask {
    pub(in crate::app) id: u64,
    pub(in crate::app) batch_id: u64,
    pub(in crate::app) direction: SftpTransferDirection,
    pub(in crate::app) local_path: PathBuf,
    pub(in crate::app) remote_path: String,
    pub(in crate::app) overwrite: bool,
    pub(in crate::app) state: SftpTransferState,
    pub(in crate::app) transferred: u64,
    pub(in crate::app) total: Option<u64>,
    pub(in crate::app) error: Option<String>,
}

pub(in crate::app) struct SftpTransferSpec {
    pub(in crate::app) batch_id: u64,
    pub(in crate::app) direction: SftpTransferDirection,
    pub(in crate::app) local_path: PathBuf,
    pub(in crate::app) remote_path: String,
    pub(in crate::app) overwrite: bool,
    pub(in crate::app) expected_total: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::app) struct SftpTransferBatchProgress {
    pub(in crate::app) task_count: usize,
    pub(in crate::app) settled_count: usize,
    pub(in crate::app) failed_count: usize,
    pub(in crate::app) transferred: u64,
    pub(in crate::app) total: Option<u64>,
    pub(in crate::app) fraction: f32,
}

impl SftpTransferTask {
    pub(in crate::app) fn display_name(&self) -> String {
        match self.direction {
            SftpTransferDirection::Upload => self
                .local_path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| self.local_path.display().to_string()),
            SftpTransferDirection::Download => remote_file_name(&self.remote_path).to_owned(),
        }
    }

    pub(in crate::app) fn status_text(&self, localizer: &Localizer) -> String {
        match self.state {
            SftpTransferState::Queued => localizer.text("sftp-queued"),
            SftpTransferState::Running => self.total.map_or_else(
                || format_remote_size(self.transferred),
                |total| {
                    format!(
                        "{} / {}",
                        format_remote_size(self.transferred),
                        format_remote_size(total)
                    )
                },
            ),
            SftpTransferState::Cancelling => localizer.text("sftp-cancelling"),
            SftpTransferState::Conflict => localizer.text("sftp-conflict"),
            SftpTransferState::Completed => {
                let mut args = fluent_bundle::FluentArgs::new();
                args.set("size", format_remote_size(self.transferred));
                localizer.text_with("sftp-completed", Some(&args))
            }
            SftpTransferState::Failed => self
                .error
                .clone()
                .unwrap_or_else(|| localizer.text("sftp-failed")),
            SftpTransferState::Cancelled => localizer.text("sftp-cancelled"),
        }
    }
}

#[derive(Default)]
pub(in crate::app) struct SftpTransferQueue {
    pub(in crate::app) next_id: u64,
    pub(in crate::app) next_batch_id: u64,
    pub(in crate::app) tasks: Vec<SftpTransferTask>,
}

impl SftpTransferQueue {
    pub(in crate::app) fn begin_batch(&mut self) -> u64 {
        self.next_batch_id = self.next_batch_id.max(1);
        let batch_id = self.next_batch_id;
        self.next_batch_id += 1;
        batch_id
    }

    pub(in crate::app) fn enqueue_in_batch(
        &mut self,
        batch_id: u64,
        direction: SftpTransferDirection,
        local_path: PathBuf,
        remote_path: String,
        overwrite: bool,
        expected_total: Option<u64>,
    ) -> u64 {
        self.next_id = self.next_id.max(1);
        let id = self.next_id;
        self.next_id += 1;
        self.tasks.push(SftpTransferTask {
            id,
            batch_id,
            direction,
            local_path,
            remote_path,
            overwrite,
            state: SftpTransferState::Queued,
            transferred: 0,
            total: expected_total,
            error: None,
        });
        id
    }

    pub(in crate::app) fn start_next(&mut self) -> Option<SftpTransferTask> {
        let task = self
            .tasks
            .iter_mut()
            .find(|task| task.state == SftpTransferState::Queued)?;
        task.state = SftpTransferState::Running;
        task.error = None;
        Some(task.clone())
    }

    pub(in crate::app) fn active_count(&self) -> usize {
        self.tasks
            .iter()
            .filter(|task| task.state.is_active())
            .count()
    }

    pub(in crate::app) fn task_mut(&mut self, id: u64) -> Option<&mut SftpTransferTask> {
        self.tasks.iter_mut().find(|task| task.id == id)
    }

    pub(in crate::app) fn mark_progress(
        &mut self,
        id: u64,
        transferred: u64,
        total: Option<u64>,
    ) -> bool {
        let Some(task) = self.task_mut(id) else {
            return false;
        };
        if !matches!(
            task.state,
            SftpTransferState::Running | SftpTransferState::Cancelling
        ) {
            return false;
        }
        task.transferred = task.transferred.max(transferred);
        if total.is_some() {
            task.total = total;
        }
        true
    }

    pub(in crate::app) fn latest_batch_progress(
        &self,
        direction: SftpTransferDirection,
    ) -> Option<SftpTransferBatchProgress> {
        let batch_id = self
            .tasks
            .iter()
            .filter(|task| task.direction == direction)
            .map(|task| task.batch_id)
            .max()?;
        let tasks = self
            .tasks
            .iter()
            .filter(|task| task.direction == direction && task.batch_id == batch_id)
            .collect::<Vec<_>>();
        if tasks.len() < 2 {
            return None;
        }

        let task_count = tasks.len();
        let settled_count = tasks.iter().filter(|task| task.state.is_finished()).count();
        let failed_count = tasks
            .iter()
            .filter(|task| {
                matches!(
                    task.state,
                    SftpTransferState::Failed | SftpTransferState::Cancelled
                )
            })
            .count();
        let all_totals_known = tasks.iter().all(|task| task.total.is_some());
        let total = all_totals_known.then(|| {
            tasks
                .iter()
                .filter_map(|task| task.total)
                .fold(0_u64, u64::saturating_add)
        });
        let transferred = tasks.iter().fold(0_u64, |sum, task| {
            sum.saturating_add(
                task.total
                    .map(|total| task.transferred.min(total))
                    .unwrap_or(task.transferred),
            )
        });
        let fraction = total
            .filter(|total| *total > 0)
            .map(|total| transferred as f32 / total as f32)
            .unwrap_or_else(|| {
                tasks
                    .iter()
                    .map(|task| {
                        if task.state == SftpTransferState::Completed {
                            1.0
                        } else {
                            task.total
                                .filter(|total| *total > 0)
                                .map(|total| task.transferred as f32 / total as f32)
                                .unwrap_or(0.0)
                        }
                    })
                    .sum::<f32>()
                    / task_count as f32
            })
            .clamp(0.0, 1.0);

        Some(SftpTransferBatchProgress {
            task_count,
            settled_count,
            failed_count,
            transferred,
            total,
            fraction,
        })
    }

    pub(in crate::app) fn latest_batch_direction(&self) -> Option<SftpTransferDirection> {
        self.tasks
            .iter()
            .max_by_key(|task| task.batch_id)
            .map(|task| task.direction)
    }

    pub(in crate::app) fn mark_conflict(&mut self, id: u64) -> bool {
        let Some(task) = self.task_mut(id) else {
            return false;
        };
        if task.state != SftpTransferState::Running {
            return false;
        }
        task.state = SftpTransferState::Conflict;
        true
    }

    pub(in crate::app) fn mark_completed(&mut self, id: u64, bytes: u64) -> bool {
        let Some(task) = self.task_mut(id) else {
            return false;
        };
        if !matches!(
            task.state,
            SftpTransferState::Running | SftpTransferState::Cancelling
        ) {
            return false;
        }
        task.state = SftpTransferState::Completed;
        task.transferred = bytes;
        task.total = Some(bytes);
        task.error = None;
        true
    }

    pub(in crate::app) fn mark_failed(&mut self, id: u64, error: String) -> bool {
        let Some(task) = self.task_mut(id) else {
            return false;
        };
        if task.state.is_finished() {
            return false;
        }
        task.state = SftpTransferState::Failed;
        task.error = Some(error);
        true
    }

    pub(in crate::app) fn mark_cancelled(&mut self, id: u64) -> bool {
        let Some(task) = self.task_mut(id) else {
            return false;
        };
        if task.state.is_finished() {
            return false;
        }
        task.state = SftpTransferState::Cancelled;
        task.error = None;
        true
    }

    pub(in crate::app) fn retry_with_overwrite(&mut self, id: u64) -> bool {
        let Some(task) = self.task_mut(id) else {
            return false;
        };
        if task.state != SftpTransferState::Conflict {
            return false;
        }
        task.overwrite = true;
        task.state = SftpTransferState::Queued;
        task.transferred = 0;
        task.total = None;
        task.error = None;
        true
    }

    pub(in crate::app) fn begin_cancel(&mut self, id: u64) -> Option<bool> {
        let task = self.task_mut(id)?;
        match task.state {
            SftpTransferState::Running => {
                task.state = SftpTransferState::Cancelling;
                Some(true)
            }
            SftpTransferState::Queued | SftpTransferState::Conflict => {
                task.state = SftpTransferState::Cancelled;
                Some(false)
            }
            SftpTransferState::Cancelling
            | SftpTransferState::Completed
            | SftpTransferState::Failed
            | SftpTransferState::Cancelled => None,
        }
    }

    pub(in crate::app) fn clear_finished(&mut self) {
        self.tasks.retain(|task| !task.state.is_finished());
    }

    pub(in crate::app) fn fail_pending(&mut self, error: &str) {
        for task in &mut self.tasks {
            if !task.state.is_finished() {
                task.state = SftpTransferState::Failed;
                task.error = Some(error.into());
            }
        }
    }
}

impl SftpFileState {
    pub(in crate::app) fn is_dirty(&self, cx: &App) -> bool {
        if !self.editable {
            return false;
        }
        self.editor
            .as_ref()
            .zip(self.text_format)
            .is_some_and(|(editor, format)| {
                format.encode(editor.read(cx).text()).as_slice() != self.original_contents
            })
    }

    pub(in crate::app) fn edited_contents(&self, cx: &App) -> Option<Vec<u8>> {
        if !self.editable {
            return None;
        }
        self.editor
            .as_ref()
            .zip(self.text_format)
            .map(|(editor, format)| format.encode(editor.read(cx).text()))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::app) struct RemoteTextFormat {
    pub(in crate::app) utf8_bom: bool,
    pub(in crate::app) line_ending: RemoteLineEnding,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::app) enum RemoteLineEnding {
    Lf,
    CrLf,
}

impl RemoteTextFormat {
    pub(in crate::app) fn decode(contents: &[u8]) -> Option<(Self, String)> {
        if contents.contains(&0) {
            return None;
        }
        let (utf8_bom, text_bytes) = contents
            .strip_prefix(&[0xef, 0xbb, 0xbf])
            .map_or((false, contents), |contents| (true, contents));
        let text = std::str::from_utf8(text_bytes).ok()?;
        let line_ending = if text.contains("\r\n") {
            RemoteLineEnding::CrLf
        } else {
            RemoteLineEnding::Lf
        };
        let text = match line_ending {
            RemoteLineEnding::Lf => text.to_owned(),
            RemoteLineEnding::CrLf => text.replace("\r\n", "\n"),
        };
        Some((
            Self {
                utf8_bom,
                line_ending,
            },
            text,
        ))
    }

    pub(in crate::app) fn encode(self, text: &str) -> Vec<u8> {
        let text = match self.line_ending {
            RemoteLineEnding::Lf => text.to_owned(),
            RemoteLineEnding::CrLf => text.replace('\n', "\r\n"),
        };
        let mut contents = Vec::with_capacity(text.len() + usize::from(self.utf8_bom) * 3);
        if self.utf8_bom {
            contents.extend_from_slice(&[0xef, 0xbb, 0xbf]);
        }
        contents.extend_from_slice(text.as_bytes());
        contents
    }
}

pub(in crate::app) fn sftp_browser_placement_for_request(request_id: u64) -> SftpBrowserPlacement {
    if request_id >= SIDEBAR_SFTP_REQUEST_ID_START {
        SftpBrowserPlacement::Sidebar
    } else {
        SftpBrowserPlacement::Center
    }
}

pub(in crate::app) fn collapse_nested_remote_entries(
    mut entries: Vec<RemoteFileEntry>,
) -> Vec<RemoteFileEntry> {
    entries.sort_by(|left, right| {
        remote_path_depth(&left.path)
            .cmp(&remote_path_depth(&right.path))
            .then_with(|| left.path.cmp(&right.path))
    });
    entries.dedup_by(|left, right| left.path == right.path);
    let selected_directories = entries
        .iter()
        .filter(|entry| entry.kind == RemoteFileKind::Directory)
        .map(|entry| entry.path.clone())
        .collect::<Vec<_>>();
    entries.retain(|entry| {
        !selected_directories.iter().any(|directory| {
            directory != &entry.path && remote_path_is_descendant(directory, &entry.path)
        })
    });
    entries
}

pub(in crate::app) fn remote_breadcrumbs(path: &str) -> Vec<(String, String)> {
    if !path.starts_with('/') {
        return vec![(path.to_owned(), path.to_owned())];
    }
    let mut breadcrumbs = vec![("/".into(), "/".into())];
    let mut target = String::new();
    for component in path.split('/').filter(|component| !component.is_empty()) {
        target.push('/');
        target.push_str(component);
        breadcrumbs.push((component.to_owned(), target.clone()));
    }
    breadcrumbs
}

pub(in crate::app) fn format_remote_size(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    let bytes = bytes as f64;

    if bytes >= GIB {
        format!("{:.1} GB", bytes / GIB)
    } else if bytes >= MIB {
        format!("{:.1} MB", bytes / MIB)
    } else if bytes >= KIB {
        format!("{:.1} KB", bytes / KIB)
    } else {
        format!("{} B", bytes as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn center_and_sidebar_sftp_requests_are_isolated() {
        let mut center = SftpBrowserState::default();
        let mut sidebar = SftpBrowserState::with_request_id_start(SIDEBAR_SFTP_REQUEST_ID_START);
        let center_request = center.begin_request("/center".into());
        let sidebar_request = sidebar.begin_request("/sidebar".into());

        assert_eq!(
            sftp_browser_placement_for_request(center_request),
            SftpBrowserPlacement::Center
        );
        assert_eq!(
            sftp_browser_placement_for_request(sidebar_request),
            SftpBrowserPlacement::Sidebar
        );
        assert!(!center.fail_request(sidebar_request, "wrong browser".into()));
        assert!(center.loading);
        assert!(sidebar.fail_request(sidebar_request, "expected".into()));
        assert!(!sidebar.loading);
    }

    #[test]
    fn remote_file_sizes_use_compact_binary_units() {
        assert_eq!(format_remote_size(42), "42 B");
        assert_eq!(format_remote_size(1536), "1.5 KB");
        assert_eq!(format_remote_size(2 * 1024 * 1024), "2.0 MB");
    }

    #[test]
    fn sftp_tree_flattens_only_expanded_directories() {
        let mut browser = SftpBrowserState {
            entries: vec![
                remote_entry("/home/test/projects", RemoteFileKind::Directory),
                remote_entry("/home/test/notes.txt", RemoteFileKind::File),
            ],
            ..SftpBrowserState::default()
        };
        browser.tree_entries.insert(
            "/home/test/projects".into(),
            vec![
                remote_entry("/home/test/projects/src", RemoteFileKind::Directory),
                remote_entry("/home/test/projects/todo.txt", RemoteFileKind::File),
            ],
        );
        browser.expanded_paths.insert("/home/test/projects".into());

        let rows = browser.visible_rows(true);
        assert_eq!(
            rows.iter()
                .map(|row| (row.entry.path.as_str(), row.depth))
                .collect::<Vec<_>>(),
            vec![
                ("/home/test/projects", 0),
                ("/home/test/projects/src", 1),
                ("/home/test/projects/todo.txt", 1),
                ("/home/test/notes.txt", 0),
            ]
        );
    }

    #[test]
    fn sftp_tree_selection_supports_ranges_and_secondary_toggle() {
        let mut browser = SftpBrowserState {
            entries: vec![
                remote_entry("/home/test/first", RemoteFileKind::File),
                remote_entry("/home/test/second", RemoteFileKind::File),
                remote_entry("/home/test/third", RemoteFileKind::File),
            ],
            ..SftpBrowserState::default()
        };

        browser.select_path("/home/test/first", gpui::Modifiers::default(), true);
        browser.select_path(
            "/home/test/third",
            gpui::Modifiers {
                shift: true,
                ..gpui::Modifiers::default()
            },
            true,
        );
        assert_eq!(
            browser.selected_paths,
            vec!["/home/test/first", "/home/test/second", "/home/test/third"]
        );

        browser.select_path("/home/test/second", secondary_modifiers_for_test(), true);
        assert_eq!(
            browser.selected_paths,
            vec!["/home/test/first", "/home/test/third"]
        );
    }

    #[test]
    fn recursive_operations_drop_children_of_selected_directories() {
        let entries = collapse_nested_remote_entries(vec![
            remote_entry("/home/test/projects/src/main.rs", RemoteFileKind::File),
            remote_entry("/home/test/notes.txt", RemoteFileKind::File),
            remote_entry("/home/test/projects", RemoteFileKind::Directory),
            remote_entry("/home/test/projects/src", RemoteFileKind::Directory),
        ]);

        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            vec!["/home/test/notes.txt", "/home/test/projects"]
        );
    }

    #[test]
    fn remote_breadcrumbs_link_every_ancestor() {
        assert_eq!(
            remote_breadcrumbs("/home/test/projects"),
            vec![
                ("/".into(), "/".into()),
                ("home".into(), "/home".into()),
                ("test".into(), "/home/test".into()),
                ("projects".into(), "/home/test/projects".into()),
            ]
        );
    }

    fn remote_entry(path: &str, kind: RemoteFileKind) -> RemoteFileEntry {
        RemoteFileEntry {
            name: remote_file_name(path).into(),
            path: path.into(),
            kind,
            size: (kind == RemoteFileKind::File).then_some(12),
            modified: None,
        }
    }

    fn secondary_modifiers_for_test() -> gpui::Modifiers {
        #[cfg(target_os = "macos")]
        {
            gpui::Modifiers {
                platform: true,
                ..gpui::Modifiers::default()
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            gpui::Modifiers {
                control: true,
                ..gpui::Modifiers::default()
            }
        }
    }

    #[test]
    fn sftp_transfer_queue_tracks_multiple_active_tasks_and_conflicts_release_slots() {
        let mut queue = SftpTransferQueue::default();
        let upload_batch = queue.begin_batch();
        let upload = queue.enqueue_in_batch(
            upload_batch,
            SftpTransferDirection::Upload,
            PathBuf::from("/tmp/upload.txt"),
            "/remote/upload.txt".into(),
            false,
            None,
        );
        let download_batch = queue.begin_batch();
        let download = queue.enqueue_in_batch(
            download_batch,
            SftpTransferDirection::Download,
            PathBuf::from("/tmp/download.txt"),
            "/remote/download.txt".into(),
            false,
            None,
        );

        assert_eq!(queue.start_next().unwrap().id, upload);
        assert_eq!(queue.start_next().unwrap().id, download);
        assert_eq!(queue.active_count(), 2);
        assert!(queue.mark_progress(upload, 9, Some(12)));
        assert!(queue.mark_progress(upload, 3, Some(12)));
        assert_eq!(queue.task_mut(upload).unwrap().transferred, 9);
        assert!(queue.mark_conflict(download));
        assert_eq!(queue.active_count(), 1);
        assert!(queue.mark_completed(upload, 12));
        assert_eq!(queue.active_count(), 0);
        assert!(queue.retry_with_overwrite(download));
        let retried = queue.start_next().unwrap();
        assert_eq!(retried.id, download);
        assert!(retried.overwrite);
    }

    #[test]
    fn queued_sftp_transfer_can_be_cancelled_without_signalling_the_worker() {
        let mut queue = SftpTransferQueue::default();
        let batch = queue.begin_batch();
        let transfer = queue.enqueue_in_batch(
            batch,
            SftpTransferDirection::Upload,
            PathBuf::from("/tmp/upload.txt"),
            "/remote/upload.txt".into(),
            false,
            None,
        );

        assert_eq!(queue.begin_cancel(transfer), Some(false));
        assert_eq!(
            queue.task_mut(transfer).map(|task| task.state),
            Some(SftpTransferState::Cancelled)
        );
    }

    #[test]
    fn multi_file_download_reports_batch_byte_progress() {
        let mut queue = SftpTransferQueue::default();
        let batch = queue.begin_batch();
        let first = queue.enqueue_in_batch(
            batch,
            SftpTransferDirection::Download,
            PathBuf::from("/tmp/first.bin"),
            "/remote/first.bin".into(),
            false,
            Some(100),
        );
        let second = queue.enqueue_in_batch(
            batch,
            SftpTransferDirection::Download,
            PathBuf::from("/tmp/second.bin"),
            "/remote/second.bin".into(),
            false,
            Some(100),
        );
        queue.enqueue_in_batch(
            batch,
            SftpTransferDirection::Download,
            PathBuf::from("/tmp/third.bin"),
            "/remote/third.bin".into(),
            false,
            Some(50),
        );

        assert_eq!(queue.start_next().map(|task| task.id), Some(first));
        assert_eq!(queue.start_next().map(|task| task.id), Some(second));
        assert!(queue.mark_completed(first, 100));
        assert!(queue.mark_progress(second, 50, Some(100)));

        assert_eq!(
            queue.latest_batch_progress(SftpTransferDirection::Download),
            Some(SftpTransferBatchProgress {
                task_count: 3,
                settled_count: 1,
                failed_count: 0,
                transferred: 150,
                total: Some(250),
                fraction: 0.6,
            })
        );
    }

    #[test]
    fn multi_file_upload_reports_total_batch_byte_progress() {
        let mut queue = SftpTransferQueue::default();
        let batch = queue.begin_batch();
        let first = queue.enqueue_in_batch(
            batch,
            SftpTransferDirection::Upload,
            PathBuf::from("/tmp/first.bin"),
            "/remote/first.bin".into(),
            false,
            Some(100),
        );
        let second = queue.enqueue_in_batch(
            batch,
            SftpTransferDirection::Upload,
            PathBuf::from("/tmp/second.bin"),
            "/remote/second.bin".into(),
            false,
            Some(300),
        );

        assert_eq!(
            queue.latest_batch_direction(),
            Some(SftpTransferDirection::Upload)
        );
        assert_eq!(queue.start_next().map(|task| task.id), Some(first));
        assert_eq!(queue.start_next().map(|task| task.id), Some(second));
        assert!(queue.mark_completed(first, 100));
        assert!(queue.mark_progress(second, 100, Some(300)));
        assert_eq!(
            queue.latest_batch_progress(SftpTransferDirection::Upload),
            Some(SftpTransferBatchProgress {
                task_count: 2,
                settled_count: 1,
                failed_count: 0,
                transferred: 200,
                total: Some(400),
                fraction: 0.5,
            })
        );
    }

    #[test]
    fn stale_sftp_error_timer_cannot_clear_a_newer_hint() {
        let mut browser = SftpBrowserState::default();
        let stale_generation = browser.set_error("first".into());
        let current_generation = browser.set_error("second".into());

        assert!(!browser.clear_error_if_current(stale_generation));
        assert_eq!(browser.error.as_deref(), Some("second"));
        assert!(browser.clear_error_if_current(current_generation));
        assert!(browser.error.is_none());
    }

    #[test]
    fn stale_sftp_response_does_not_replace_the_latest_directory() {
        let mut browser = SftpBrowserState::default();
        let stale_request = browser.begin_request("/stale".into());
        let current_request = browser.begin_request("/current".into());

        assert!(!browser.complete_request(
            stale_request,
            RemoteDirectory {
                path: "/stale".into(),
                entries: Vec::new(),
            },
        ));
        assert!(browser.complete_request(
            current_request,
            RemoteDirectory {
                path: "/current".into(),
                entries: Vec::new(),
            },
        ));
        assert_eq!(browser.path, "/current");
        assert_eq!(browser.resolved_source_path.as_deref(), Some("/current"));
        assert!(!browser.loading);
    }

    #[test]
    fn canonical_sftp_result_remains_linked_to_its_shell_cwd_request() {
        let mut browser = SftpBrowserState::default();
        assert!(browser.needs_request("."));

        let request = browser.begin_request(".".into());
        assert!(!browser.needs_request("."));
        assert!(browser.complete_request(
            request,
            RemoteDirectory {
                path: "/home/test".into(),
                entries: Vec::new(),
            }
        ));

        assert!(!browser.needs_request("."));
        assert!(browser.needs_request("/var/log"));
    }

    #[test]
    fn remote_text_format_preserves_utf8_bom_and_crlf() {
        let contents = b"\xef\xbb\xbffirst\r\nsecond\r\n";
        let (format, text) = RemoteTextFormat::decode(contents).expect("UTF-8 text");

        assert_eq!(text, "first\nsecond\n");
        assert_eq!(format.line_ending, RemoteLineEnding::CrLf);
        assert!(format.utf8_bom);
        assert_eq!(format.encode(&text), contents);
    }

    #[test]
    fn remote_text_format_rejects_binary_and_invalid_utf8() {
        assert!(RemoteTextFormat::decode(b"text\0data").is_none());
        assert!(RemoteTextFormat::decode(&[0xff, 0xfe]).is_none());
    }
}
