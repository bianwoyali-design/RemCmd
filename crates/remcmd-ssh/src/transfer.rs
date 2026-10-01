use crate::{ConnectionEvent, SftpOperation, SftpTransferDirection, SshError, SshErrorKind};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::{
    sync::{Mutex as AsyncMutex, mpsc},
    time::{Duration, sleep},
};

mod plan;
pub use plan::{LocalUploadPlan, build_local_upload_plan, build_remote_download_plan};

pub(crate) const TRANSFER_CHUNK_BYTES: usize = 128 * 1024;
static NEXT_TRANSFER_TEMPORARY_ID: AtomicU64 = AtomicU64::new(1);

pub struct TransferRateLimiter {
    bytes_per_second: AtomicU64,
    pacing: AsyncMutex<()>,
}

impl TransferRateLimiter {
    pub fn new(bytes_per_second: Option<u64>) -> Self {
        Self {
            bytes_per_second: AtomicU64::new(bytes_per_second.unwrap_or(0)),
            pacing: AsyncMutex::new(()),
        }
    }

    pub fn set_bytes_per_second(&self, bytes_per_second: Option<u64>) {
        self.bytes_per_second
            .store(bytes_per_second.unwrap_or(0), Ordering::Release);
    }

    async fn acquire(&self, bytes: usize) {
        if bytes == 0 || self.bytes_per_second.load(Ordering::Acquire) == 0 {
            return;
        }

        let _pacing = self.pacing.lock().await;
        let bytes_per_second = self.bytes_per_second.load(Ordering::Acquire);
        if bytes_per_second == 0 {
            return;
        }
        sleep(Duration::from_secs_f64(
            bytes as f64 / bytes_per_second as f64,
        ))
        .await;
    }
}

impl Default for TransferRateLimiter {
    fn default() -> Self {
        Self::new(None)
    }
}

pub(crate) enum TransferResult {
    Completed(u64),
    Conflict,
    Cancelled,
}

#[derive(Clone)]
pub(crate) struct TransferContext {
    transfer_id: u64,
    cancellation: Arc<AtomicBool>,
    events: mpsc::Sender<ConnectionEvent>,
    rate_limiter: Arc<TransferRateLimiter>,
}

impl TransferContext {
    pub(crate) fn new(
        transfer_id: u64,
        cancellation: Arc<AtomicBool>,
        events: mpsc::Sender<ConnectionEvent>,
        rate_limiter: Arc<TransferRateLimiter>,
    ) -> Self {
        Self {
            transfer_id,
            cancellation,
            events,
            rate_limiter,
        }
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancellation.load(Ordering::Acquire)
    }

    pub(crate) const fn transfer_id(&self) -> u64 {
        self.transfer_id
    }

    pub(crate) async fn acquire_rate_budget(&self, bytes: usize) {
        self.rate_limiter.acquire(bytes).await;
    }

    pub(crate) async fn report_progress(
        &self,
        transferred: u64,
        total: Option<u64>,
    ) -> Result<(), SshError> {
        self.events
            .send(ConnectionEvent::TransferProgress {
                transfer_id: self.transfer_id,
                transferred,
                total,
            })
            .await
            .map_err(|_| {
                SshError::new(
                    SshErrorKind::InvalidState,
                    "SSH connection event receiver is not running",
                )
            })
    }
}

pub(crate) fn transfer_result_event(
    transfer_id: u64,
    path: String,
    direction: SftpTransferDirection,
    result: Result<TransferResult, SshError>,
) -> ConnectionEvent {
    match result {
        Ok(TransferResult::Completed(bytes)) => ConnectionEvent::TransferCompleted {
            transfer_id,
            direction,
            path,
            bytes,
        },
        Ok(TransferResult::Conflict) => ConnectionEvent::TransferConflict {
            transfer_id,
            direction,
            path,
        },
        Ok(TransferResult::Cancelled) => ConnectionEvent::TransferCancelled { transfer_id },
        Err(error) => ConnectionEvent::SftpFailed {
            request_id: transfer_id,
            path,
            operation: match direction {
                SftpTransferDirection::Upload => SftpOperation::UploadFile,
                SftpTransferDirection::Download => SftpOperation::DownloadFile,
            },
            error,
        },
    }
}

pub(crate) fn transfer_temporary_suffix(transfer_id: u64) -> String {
    let sequence = NEXT_TRANSFER_TEMPORARY_ID.fetch_add(1, Ordering::Relaxed);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!(
        "{transfer_id:x}-{:x}-{timestamp:x}-{sequence:x}",
        std::process::id()
    )
}

pub(crate) fn remote_transfer_temporary_path(path: &str, suffix: &str) -> String {
    format!("{path}.remcmd-{suffix}.part")
}

pub(crate) fn local_transfer_temporary_path(path: &Path, suffix: &str) -> PathBuf {
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_else(|| "download".into());
    path.with_file_name(format!(".{file_name}.remcmd-{suffix}.part"))
}

pub(crate) fn transfer_io_error(action: &str, error: std::io::Error) -> SshError {
    SshError::new(SshErrorKind::Sftp, format!("{action}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::timeout;

    #[tokio::test]
    async fn rate_limiter_shares_one_budget_between_concurrent_transfers() {
        let rate_limiter = Arc::new(TransferRateLimiter::new(Some(1024 * 1024)));
        let started = tokio::time::Instant::now();

        tokio::join!(
            rate_limiter.acquire(64 * 1024),
            rate_limiter.acquire(64 * 1024)
        );

        assert!(started.elapsed() >= Duration::from_millis(120));
        rate_limiter.set_bytes_per_second(None);
        timeout(Duration::from_millis(20), rate_limiter.acquire(64 * 1024))
            .await
            .expect("disabling the rate limit should take effect immediately");
    }
}
