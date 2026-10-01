use std::{
    ops::Range,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use russh_sftp::{
    client::SftpSession,
    protocol::{FileType, OpenFlags},
};
use tokio::{
    fs,
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
    task::JoinSet,
};

use crate::remote_path::remote_path_depth;
use crate::transfer::{
    TRANSFER_CHUNK_BYTES, TransferContext, TransferResult, local_transfer_temporary_path,
    remote_transfer_temporary_path, transfer_io_error, transfer_temporary_suffix,
};
use crate::{SshError, SshErrorKind};

pub const MAX_REMOTE_FILE_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SftpOperation {
    ReadDirectory,
    ReadDirectoryTree,
    ReadFile,
    WriteFile,
    CreateFile,
    CreateDirectory,
    DeletePaths,
    UploadFile,
    DownloadFile,
    CancelTransfer,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SftpTransferDirection {
    Upload,
    Download,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RemoteFileKind {
    Directory,
    File,
    Symlink,
    Other,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteFileEntry {
    pub name: String,
    pub path: String,
    pub kind: RemoteFileKind,
    pub size: Option<u64>,
    pub modified: Option<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteDirectory {
    pub path: String,
    pub entries: Vec<RemoteFileEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteDirectoryTree {
    pub root: String,
    pub directories: Vec<String>,
    pub files: Vec<RemoteFileEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteFile {
    pub path: String,
    pub contents: Vec<u8>,
}

const DOWNLOAD_PIPELINE_STREAMS: usize = 4;
const MIN_DOWNLOAD_SEGMENT_BYTES: u64 = 1024 * 1024;

pub(crate) async fn upload_file(
    session: &SftpSession,
    local_path: &Path,
    remote_path: String,
    overwrite: bool,
    context: &TransferContext,
) -> Result<TransferResult, SshError> {
    let metadata = fs::metadata(local_path)
        .await
        .map_err(|error| transfer_io_error("reading local file metadata", error))?;
    if !metadata.is_file() {
        return Err(SshError::new(
            SshErrorKind::Sftp,
            "Only regular files can be uploaded",
        ));
    }
    let total = metadata.len();

    if session
        .try_exists(remote_path.clone())
        .await
        .map_err(SshError::from)?
        && !overwrite
    {
        return Ok(TransferResult::Conflict);
    }

    if context.is_cancelled() {
        return Ok(TransferResult::Cancelled);
    }

    let temporary_path = remote_transfer_temporary_path(
        &remote_path,
        &transfer_temporary_suffix(context.transfer_id()),
    );
    if session
        .try_exists(temporary_path.clone())
        .await
        .map_err(SshError::from)?
    {
        session
            .remove_file(temporary_path.clone())
            .await
            .map_err(SshError::from)?;
    }

    let copy_result = copy_upload(session, local_path, &temporary_path, total, context).await;
    let transferred = match copy_result {
        Ok(TransferResult::Completed(bytes)) => bytes,
        Ok(TransferResult::Cancelled) => {
            let _ = session.remove_file(temporary_path).await;
            return Ok(TransferResult::Cancelled);
        }
        Ok(TransferResult::Conflict) => unreachable!("copy cannot report a conflict"),
        Err(error) => {
            let _ = session.remove_file(temporary_path).await;
            return Err(error);
        }
    };

    if context.is_cancelled() {
        let _ = session.remove_file(temporary_path).await;
        return Ok(TransferResult::Cancelled);
    }

    let install_result = async {
        if overwrite
            && session
                .try_exists(remote_path.clone())
                .await
                .map_err(SshError::from)?
        {
            session
                .remove_file(remote_path.clone())
                .await
                .map_err(SshError::from)?;
        }
        session
            .rename(temporary_path.clone(), remote_path)
            .await
            .map_err(SshError::from)
    }
    .await;
    if let Err(error) = install_result {
        let _ = session.remove_file(temporary_path).await;
        return Err(error);
    }

    Ok(TransferResult::Completed(transferred))
}

async fn copy_upload(
    session: &SftpSession,
    local_path: &Path,
    temporary_path: &str,
    total: u64,
    context: &TransferContext,
) -> Result<TransferResult, SshError> {
    let mut local_file = fs::File::open(local_path)
        .await
        .map_err(|error| transfer_io_error("opening local file", error))?;
    let mut remote_file = session
        .create(temporary_path.to_owned())
        .await
        .map_err(SshError::from)?;
    let mut buffer = vec![0; TRANSFER_CHUNK_BYTES];
    let mut transferred = 0_u64;

    loop {
        if context.is_cancelled() {
            return Ok(TransferResult::Cancelled);
        }
        let read = local_file
            .read(&mut buffer)
            .await
            .map_err(|error| transfer_io_error("reading local file", error))?;
        if read == 0 {
            break;
        }
        context.acquire_rate_budget(read).await;
        if context.is_cancelled() {
            return Ok(TransferResult::Cancelled);
        }
        remote_file
            .write_all(&buffer[..read])
            .await
            .map_err(|error| transfer_io_error("writing remote file", error))?;
        transferred += read as u64;
        context.report_progress(transferred, Some(total)).await?;
    }

    remote_file.sync_all().await.map_err(SshError::from)?;
    remote_file
        .shutdown()
        .await
        .map_err(|error| transfer_io_error("closing remote file", error))?;
    Ok(TransferResult::Completed(transferred))
}

pub(crate) async fn download_file(
    session: Arc<SftpSession>,
    remote_path: String,
    local_path: &Path,
    overwrite: bool,
    context: &TransferContext,
) -> Result<TransferResult, SshError> {
    if fs::try_exists(local_path)
        .await
        .map_err(|error| transfer_io_error("checking local destination", error))?
        && !overwrite
    {
        return Ok(TransferResult::Conflict);
    }

    if context.is_cancelled() {
        return Ok(TransferResult::Cancelled);
    }

    let remote_path = session
        .canonicalize(remote_path)
        .await
        .map_err(SshError::from)?;
    let metadata = session
        .metadata(remote_path.clone())
        .await
        .map_err(SshError::from)?;
    let total = metadata.size;
    let temporary_path = local_transfer_temporary_path(
        local_path,
        &transfer_temporary_suffix(context.transfer_id()),
    );
    if fs::try_exists(&temporary_path)
        .await
        .map_err(|error| transfer_io_error("checking temporary download", error))?
    {
        fs::remove_file(&temporary_path)
            .await
            .map_err(|error| transfer_io_error("removing stale temporary download", error))?;
    }

    let copy_result = copy_download(
        session,
        remote_path,
        temporary_path.clone(),
        total,
        context.clone(),
    )
    .await;
    let transferred = match copy_result {
        Ok(TransferResult::Completed(bytes)) => bytes,
        Ok(TransferResult::Cancelled) => {
            let _ = fs::remove_file(&temporary_path).await;
            return Ok(TransferResult::Cancelled);
        }
        Ok(TransferResult::Conflict) => unreachable!("copy cannot report a conflict"),
        Err(error) => {
            let _ = fs::remove_file(&temporary_path).await;
            return Err(error);
        }
    };

    if context.is_cancelled() {
        let _ = fs::remove_file(&temporary_path).await;
        return Ok(TransferResult::Cancelled);
    }

    let install_result = async {
        if overwrite
            && fs::try_exists(local_path)
                .await
                .map_err(|error| transfer_io_error("checking local destination", error))?
        {
            fs::remove_file(local_path)
                .await
                .map_err(|error| transfer_io_error("replacing local destination", error))?;
        }
        fs::rename(&temporary_path, local_path)
            .await
            .map_err(|error| transfer_io_error("installing downloaded file", error))
    }
    .await;
    if let Err(error) = install_result {
        let _ = fs::remove_file(&temporary_path).await;
        return Err(error);
    }

    Ok(TransferResult::Completed(transferred))
}

async fn copy_download(
    session: Arc<SftpSession>,
    remote_path: String,
    temporary_path: PathBuf,
    total: Option<u64>,
    context: TransferContext,
) -> Result<TransferResult, SshError> {
    let ranges = total.map(download_segment_ranges).unwrap_or_default();
    if ranges.len() > 1 {
        return copy_download_parallel(
            session,
            remote_path,
            temporary_path,
            total.expect("parallel download ranges require a known size"),
            ranges,
            context,
        )
        .await;
    }

    copy_download_sequential(&session, &remote_path, &temporary_path, total, &context).await
}

async fn copy_download_sequential(
    session: &SftpSession,
    remote_path: &str,
    temporary_path: &Path,
    total: Option<u64>,
    context: &TransferContext,
) -> Result<TransferResult, SshError> {
    let mut remote_file = session
        .open(remote_path.to_owned())
        .await
        .map_err(SshError::from)?;
    let mut local_file = fs::File::create(temporary_path)
        .await
        .map_err(|error| transfer_io_error("creating temporary download", error))?;
    let mut buffer = vec![0; TRANSFER_CHUNK_BYTES];
    let mut transferred = 0_u64;

    loop {
        if context.is_cancelled() {
            return Ok(TransferResult::Cancelled);
        }
        let read = remote_file
            .read(&mut buffer)
            .await
            .map_err(|error| transfer_io_error("reading remote file", error))?;
        if read == 0 {
            break;
        }
        context.acquire_rate_budget(read).await;
        if context.is_cancelled() {
            return Ok(TransferResult::Cancelled);
        }
        local_file
            .write_all(&buffer[..read])
            .await
            .map_err(|error| transfer_io_error("writing temporary download", error))?;
        transferred += read as u64;
        context.report_progress(transferred, total).await?;
    }

    local_file
        .sync_all()
        .await
        .map_err(|error| transfer_io_error("syncing downloaded file", error))?;
    Ok(TransferResult::Completed(transferred))
}

async fn copy_download_parallel(
    session: Arc<SftpSession>,
    remote_path: String,
    temporary_path: PathBuf,
    total: u64,
    ranges: Vec<Range<u64>>,
    context: TransferContext,
) -> Result<TransferResult, SshError> {
    let local_file = fs::File::create(&temporary_path)
        .await
        .map_err(|error| transfer_io_error("creating temporary download", error))?;
    local_file
        .set_len(total)
        .await
        .map_err(|error| transfer_io_error("preallocating temporary download", error))?;
    drop(local_file);

    let transferred = Arc::new(AtomicU64::new(0));
    let mut segments = JoinSet::new();
    for range in ranges {
        segments.spawn(copy_download_segment(
            session.clone(),
            remote_path.clone(),
            temporary_path.clone(),
            range,
            total,
            context.clone(),
            transferred.clone(),
        ));
    }

    let mut completed_bytes = 0_u64;
    while let Some(result) = segments.join_next().await {
        let result = result.unwrap_or_else(|error| {
            Err(SshError::new(
                SshErrorKind::Sftp,
                format!("parallel download task failed: {error}"),
            ))
        });
        match result {
            Ok(TransferResult::Completed(bytes)) => {
                completed_bytes += bytes;
            }
            Ok(TransferResult::Conflict) => {
                unreachable!("download segments cannot report a conflict");
            }
            result => {
                segments.shutdown().await;
                return result;
            }
        }
    }

    let local_file = fs::OpenOptions::new()
        .write(true)
        .open(&temporary_path)
        .await
        .map_err(|error| transfer_io_error("opening completed download", error))?;
    local_file
        .sync_all()
        .await
        .map_err(|error| transfer_io_error("syncing downloaded file", error))?;
    Ok(TransferResult::Completed(completed_bytes))
}

async fn copy_download_segment(
    session: Arc<SftpSession>,
    remote_path: String,
    temporary_path: PathBuf,
    range: Range<u64>,
    total: u64,
    context: TransferContext,
    transferred: Arc<AtomicU64>,
) -> Result<TransferResult, SshError> {
    let mut remote_file = session.open(remote_path).await.map_err(SshError::from)?;
    remote_file
        .seek(std::io::SeekFrom::Start(range.start))
        .await
        .map_err(|error| transfer_io_error("seeking remote file", error))?;
    let mut local_file = fs::OpenOptions::new()
        .write(true)
        .open(temporary_path)
        .await
        .map_err(|error| transfer_io_error("opening temporary download segment", error))?;
    local_file
        .seek(std::io::SeekFrom::Start(range.start))
        .await
        .map_err(|error| transfer_io_error("seeking temporary download segment", error))?;

    let mut remaining = range.end - range.start;
    let mut segment_bytes = 0_u64;
    let mut buffer = vec![0; TRANSFER_CHUNK_BYTES];
    while remaining > 0 {
        if context.is_cancelled() {
            return Ok(TransferResult::Cancelled);
        }
        let requested = usize::try_from(remaining.min(TRANSFER_CHUNK_BYTES as u64))
            .expect("download segment chunk fits usize");
        let read = remote_file
            .read(&mut buffer[..requested])
            .await
            .map_err(|error| transfer_io_error("reading remote download segment", error))?;
        if read == 0 {
            return Err(SshError::new(
                SshErrorKind::Sftp,
                "remote file ended before the download segment completed",
            ));
        }
        context.acquire_rate_budget(read).await;
        if context.is_cancelled() {
            return Ok(TransferResult::Cancelled);
        }
        local_file
            .write_all(&buffer[..read])
            .await
            .map_err(|error| transfer_io_error("writing temporary download segment", error))?;
        remaining -= read as u64;
        segment_bytes += read as u64;
        let aggregate = transferred.fetch_add(read as u64, Ordering::AcqRel) + read as u64;
        context.report_progress(aggregate, Some(total)).await?;
    }
    local_file
        .flush()
        .await
        .map_err(|error| transfer_io_error("flushing temporary download segment", error))?;
    remote_file
        .shutdown()
        .await
        .map_err(|error| transfer_io_error("closing remote download segment", error))?;
    Ok(TransferResult::Completed(segment_bytes))
}

fn download_segment_ranges(total: u64) -> Vec<Range<u64>> {
    if total == 0 {
        return Vec::new();
    }
    let stream_count = usize::try_from(total.div_ceil(MIN_DOWNLOAD_SEGMENT_BYTES))
        .unwrap_or(usize::MAX)
        .clamp(1, DOWNLOAD_PIPELINE_STREAMS);
    let segment_size = total.div_ceil(stream_count as u64);
    (0..stream_count)
        .map(|index| {
            let start = index as u64 * segment_size;
            start..(start + segment_size).min(total)
        })
        .filter(|range| range.start < range.end)
        .collect()
}

pub(crate) async fn read_directory(
    session: &SftpSession,
    path: String,
) -> Result<RemoteDirectory, SshError> {
    let path = session.canonicalize(path).await.map_err(SshError::from)?;
    let entries = session
        .read_dir(path.clone())
        .await
        .map_err(SshError::from)?;
    let mut entries = entries
        .map(|entry| {
            let metadata = entry.metadata();
            RemoteFileEntry {
                name: entry.file_name(),
                path: entry.path(),
                kind: remote_file_kind(entry.file_type()),
                size: metadata.size,
                modified: metadata.mtime,
            }
        })
        .collect::<Vec<_>>();
    sort_entries(&mut entries);

    Ok(RemoteDirectory { path, entries })
}

pub(crate) async fn read_directory_tree(
    session: &SftpSession,
    path: String,
) -> Result<RemoteDirectoryTree, SshError> {
    let root = session.canonicalize(path).await.map_err(SshError::from)?;
    let mut pending = vec![root.clone()];
    let mut directories = Vec::new();
    let mut files = Vec::new();

    while let Some(directory) = pending.pop() {
        let entries = session.read_dir(directory).await.map_err(SshError::from)?;
        for entry in entries {
            let metadata = entry.metadata();
            match remote_file_kind(entry.file_type()) {
                RemoteFileKind::Directory => {
                    let path = entry.path();
                    directories.push(path.clone());
                    pending.push(path);
                }
                RemoteFileKind::File | RemoteFileKind::Other if metadata.size.is_some() => {
                    files.push(RemoteFileEntry {
                        name: entry.file_name(),
                        path: entry.path(),
                        kind: RemoteFileKind::File,
                        size: metadata.size,
                        modified: metadata.mtime,
                    });
                }
                RemoteFileKind::File | RemoteFileKind::Symlink | RemoteFileKind::Other => {}
            }
        }
    }
    directories.sort_by(|left, right| {
        remote_path_depth(left)
            .cmp(&remote_path_depth(right))
            .then_with(|| left.cmp(right))
    });
    files.sort_by(|left, right| left.path.cmp(&right.path));

    Ok(RemoteDirectoryTree {
        root,
        directories,
        files,
    })
}

pub(crate) async fn create_file(session: &SftpSession, path: String) -> Result<String, SshError> {
    let mut file = session
        .open_with_flags(
            path.clone(),
            OpenFlags::CREATE | OpenFlags::EXCLUDE | OpenFlags::WRITE,
        )
        .await
        .map_err(SshError::from)?;
    file.sync_all().await.map_err(SshError::from)?;
    file.shutdown()
        .await
        .map_err(|error| transfer_io_error("closing new remote file", error))?;
    session.canonicalize(path).await.map_err(SshError::from)
}

pub(crate) async fn create_directories(
    session: &SftpSession,
    mut paths: Vec<String>,
) -> Result<(), SshError> {
    paths.sort_by(|left, right| {
        remote_path_depth(left)
            .cmp(&remote_path_depth(right))
            .then_with(|| left.cmp(right))
    });
    paths.dedup();

    for path in paths {
        if session
            .try_exists(path.clone())
            .await
            .map_err(SshError::from)?
        {
            let metadata = session
                .symlink_metadata(path)
                .await
                .map_err(SshError::from)?;
            if !metadata.file_type().is_dir() {
                return Err(SshError::new(
                    SshErrorKind::Sftp,
                    "Cannot create a directory over an existing remote file",
                ));
            }
            continue;
        }
        session.create_dir(path).await.map_err(SshError::from)?;
    }
    Ok(())
}

pub(crate) async fn delete_paths(session: &SftpSession, paths: &[String]) -> Result<(), SshError> {
    enum DeleteStep {
        Inspect(String),
        RemoveDirectory(String),
    }

    for requested_path in paths {
        let path = session
            .canonicalize(requested_path.clone())
            .await
            .map_err(SshError::from)?;
        if path == "/" {
            return Err(SshError::new(
                SshErrorKind::Sftp,
                "Refusing to delete the remote root directory",
            ));
        }

        let mut pending = vec![DeleteStep::Inspect(path)];
        while let Some(step) = pending.pop() {
            match step {
                DeleteStep::Inspect(path) => {
                    let metadata =
                        session
                            .symlink_metadata(path.clone())
                            .await
                            .map_err(|error| {
                                SshError::new(
                                    SshErrorKind::Sftp,
                                    format!("reading metadata for {path}: {error}"),
                                )
                            })?;
                    if metadata.file_type().is_dir() {
                        let entries = session.read_dir(path.clone()).await.map_err(|error| {
                            SshError::new(
                                SshErrorKind::Sftp,
                                format!("reading directory {path}: {error}"),
                            )
                        })?;
                        pending.push(DeleteStep::RemoveDirectory(path));
                        pending.extend(
                            entries
                                .map(|entry| DeleteStep::Inspect(entry.path()))
                                .collect::<Vec<_>>()
                                .into_iter()
                                .rev(),
                        );
                    } else {
                        session.remove_file(path.clone()).await.map_err(|error| {
                            SshError::new(
                                SshErrorKind::Sftp,
                                format!("deleting file {path}: {error}"),
                            )
                        })?;
                    }
                }
                DeleteStep::RemoveDirectory(path) => {
                    session.remove_dir(path.clone()).await.map_err(|error| {
                        SshError::new(
                            SshErrorKind::Sftp,
                            format!("deleting directory {path}: {error}"),
                        )
                    })?;
                }
            }
        }
    }
    Ok(())
}

pub(crate) async fn read_file(session: &SftpSession, path: String) -> Result<RemoteFile, SshError> {
    let path = session.canonicalize(path).await.map_err(SshError::from)?;
    let metadata = session
        .metadata(path.clone())
        .await
        .map_err(SshError::from)?;
    if metadata
        .size
        .is_some_and(|size| size > MAX_REMOTE_FILE_BYTES as u64)
    {
        return Err(file_too_large_error());
    }

    let file = session.open(path.clone()).await.map_err(SshError::from)?;
    let mut contents = Vec::with_capacity(metadata.size.unwrap_or_default() as usize);
    file.take((MAX_REMOTE_FILE_BYTES + 1) as u64)
        .read_to_end(&mut contents)
        .await
        .map_err(|error| SshError::new(SshErrorKind::Sftp, error.to_string()))?;
    if contents.len() > MAX_REMOTE_FILE_BYTES {
        return Err(file_too_large_error());
    }

    Ok(RemoteFile { path, contents })
}

pub(crate) async fn write_file(
    session: &SftpSession,
    path: String,
    expected_contents: Vec<u8>,
    contents: Vec<u8>,
) -> Result<RemoteFile, SshError> {
    if contents.len() > MAX_REMOTE_FILE_BYTES {
        return Err(file_too_large_error());
    }

    let current = read_file(session, path).await?;
    if current.contents != expected_contents {
        return Err(SshError::new(
            SshErrorKind::Sftp,
            "Remote file changed since it was opened. Reload it before saving.",
        ));
    }

    let mut file = session
        .create(current.path.clone())
        .await
        .map_err(SshError::from)?;
    file.write_all(&contents)
        .await
        .map_err(|error| SshError::new(SshErrorKind::Sftp, error.to_string()))?;
    file.sync_all().await.map_err(SshError::from)?;
    file.shutdown()
        .await
        .map_err(|error| SshError::new(SshErrorKind::Sftp, error.to_string()))?;

    Ok(RemoteFile {
        path: current.path,
        contents,
    })
}

fn file_too_large_error() -> SshError {
    SshError::new(
        SshErrorKind::Sftp,
        format!(
            "Remote file is larger than the {} MB editor limit",
            MAX_REMOTE_FILE_BYTES / 1024 / 1024
        ),
    )
}

fn remote_file_kind(kind: FileType) -> RemoteFileKind {
    match kind {
        FileType::Dir => RemoteFileKind::Directory,
        FileType::File => RemoteFileKind::File,
        FileType::Symlink => RemoteFileKind::Symlink,
        FileType::Other => RemoteFileKind::Other,
    }
}

fn sort_entries(entries: &mut [RemoteFileEntry]) {
    entries.sort_by(|left, right| {
        file_kind_rank(left.kind)
            .cmp(&file_kind_rank(right.kind))
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
            .then_with(|| left.name.cmp(&right.name))
    });
}

const fn file_kind_rank(kind: RemoteFileKind) -> u8 {
    match kind {
        RemoteFileKind::Directory => 0,
        RemoteFileKind::Symlink => 1,
        RemoteFileKind::File => 2,
        RemoteFileKind::Other => 3,
    }
}

#[cfg(test)]
mod tests;
