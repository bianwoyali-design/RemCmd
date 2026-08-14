use std::{
    collections::{BTreeMap, HashSet, VecDeque},
    fmt, fs,
    fs::{File, OpenOptions},
    io::{self, BufRead, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    process,
    sync::{
        Arc, LazyLock, Mutex, RwLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration as StdDuration, Instant, SystemTime, UNIX_EPOCH},
};

use chrono::{Duration, NaiveDate, SecondsFormat, Utc};
use directories::{BaseDirs, ProjectDirs};
use regex::{Captures, Regex};
use remcmd_core::{ConnectionProfile, LanguageMode, TabLayout, ThemeMode};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::{Event, Level, Metadata, Subscriber, field::Visit};
use tracing_subscriber::{Layer, layer::Context, prelude::*};

const DEFAULT_EVENT_CAPACITY: usize = 2_000;
const LOG_RETENTION_DAYS: i64 = 7;
const LOG_BUFFER_CAPACITY: usize = 16 * 1_024;
const LOG_FLUSH_EVENT_INTERVAL: usize = 32;
const LOG_FLUSH_INTERVAL: StdDuration = StdDuration::from_secs(1);
const REDACTED: &str = "[REDACTED]";

static FALLBACK_DIRECTORY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

static URI_USERINFO: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)([a-z][a-z0-9+.-]*://)([^/@\s]+)@").expect("URI redaction pattern is valid")
});
static AUTHORIZATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(authorization\s*[:=]\s*)([^\s,;]+)")
        .expect("authorization redaction pattern is valid")
});
static NAMED_SECRET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)((?:password|passphrase|token|secret)\s*[:=]\s*)(?:\"[^\"]*\"|'[^']*'|[^\s,;}]+)"#,
    )
    .expect("named secret redaction pattern is valid")
});
static SENSITIVE_FIELD_NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:authorization|password|passphrase|token|secret)")
        .expect("sensitive field-name pattern is valid")
});

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DiagnosticLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl DiagnosticLevel {
    fn priority(self) -> u8 {
        match self {
            Self::Error => 1,
            Self::Warn => 2,
            Self::Info => 3,
            Self::Debug => 4,
            Self::Trace => 5,
        }
    }

    fn from_tracing(level: &Level) -> Self {
        match *level {
            Level::ERROR => Self::Error,
            Level::WARN => Self::Warn,
            Level::INFO => Self::Info,
            Level::DEBUG => Self::Debug,
            Level::TRACE => Self::Trace,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct DiagnosticEvent {
    pub timestamp: String,
    pub level: DiagnosticLevel,
    pub module: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fields: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiagnosticFilter {
    pub level: Option<DiagnosticLevel>,
    pub module: String,
    pub text: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SupportBundleContext {
    pub app_version: String,
    pub os: String,
    pub architecture: String,
    pub language: LanguageMode,
    pub theme: ThemeMode,
    pub tab_layout: TabLayout,
    pub terminal_font_size: u16,
    pub transfer_rate_limit_mib_per_second: u32,
    pub max_parallel_transfers: u8,
}

impl SupportBundleContext {
    pub fn for_current_platform(app_version: impl Into<String>) -> Self {
        Self {
            app_version: app_version.into(),
            os: std::env::consts::OS.into(),
            architecture: std::env::consts::ARCH.into(),
            language: LanguageMode::System,
            theme: ThemeMode::System,
            tab_layout: TabLayout::default(),
            terminal_font_size: remcmd_core::DEFAULT_TERMINAL_FONT_SIZE,
            transfer_rate_limit_mib_per_second: 0,
            max_parallel_transfers: remcmd_core::DEFAULT_MAX_PARALLEL_TRANSFERS,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
struct SupportBundleManifest {
    generated_at: String,
    context: SupportBundleContext,
    profiles: Vec<AnonymousProfile>,
}

#[derive(Clone, Debug, Serialize)]
struct AnonymousProfile {
    id_hash: String,
    endpoint_hash: String,
    uses_default_ssh_port: bool,
    authentication: &'static str,
    proxy: &'static str,
    jump_count: usize,
    imported_from_openssh: bool,
}

#[derive(Clone, Default)]
pub struct Redactor {
    secrets: Arc<RwLock<Vec<String>>>,
}

impl Redactor {
    pub fn register(&self, secret: &SecretString) {
        self.register_text(secret.expose_secret());
    }

    pub fn register_text(&self, secret: &str) {
        if secret.is_empty() {
            return;
        }
        let mut secrets = self
            .secrets
            .write()
            .unwrap_or_else(|error| error.into_inner());
        if !secrets.iter().any(|known| known == secret) {
            secrets.push(secret.to_owned());
            secrets.sort_by_key(|secret| std::cmp::Reverse(secret.len()));
        }
    }

    pub fn redact(&self, input: &str) -> String {
        let mut redacted = input.to_owned();
        let secrets = self
            .secrets
            .read()
            .unwrap_or_else(|error| error.into_inner());
        for secret in secrets.iter() {
            redacted = redacted.replace(secret, REDACTED);
        }
        drop(secrets);

        redacted = URI_USERINFO
            .replace_all(&redacted, |captures: &Captures<'_>| {
                format!("{}{REDACTED}@", &captures[1])
            })
            .into_owned();
        redacted = AUTHORIZATION
            .replace_all(&redacted, |captures: &Captures<'_>| {
                format!("{}{REDACTED}", &captures[1])
            })
            .into_owned();
        NAMED_SECRET
            .replace_all(&redacted, |captures: &Captures<'_>| {
                format!("{}{REDACTED}", &captures[1])
            })
            .into_owned()
    }
}

#[derive(Clone)]
pub struct DiagnosticStore {
    inner: Arc<DiagnosticInner>,
}

struct DiagnosticInner {
    events: Mutex<VecDeque<DiagnosticEvent>>,
    capacity: usize,
    log_dir: PathBuf,
    file: Mutex<FileState>,
    redactor: Redactor,
    debug_enabled: AtomicBool,
}

struct FileState {
    date: Option<NaiveDate>,
    file: Option<BufWriter<File>>,
    pending_lines: usize,
    last_flush: Instant,
    error: Option<String>,
}

impl DiagnosticStore {
    pub fn new(log_dir: impl Into<PathBuf>) -> Self {
        Self::with_capacity(log_dir, DEFAULT_EVENT_CAPACITY)
    }

    pub fn with_capacity(log_dir: impl Into<PathBuf>, capacity: usize) -> Self {
        let log_dir = log_dir.into();
        let mut error = None;
        if let Err(failure) = create_private_log_directory(&log_dir) {
            error = Some(format!("Failed to initialize diagnostic logs: {failure}"));
        } else if let Err(failure) = cleanup_old_logs(&log_dir, Utc::now().date_naive()) {
            error = Some(format!("Failed to clean old diagnostic logs: {failure}"));
        }
        Self {
            inner: Arc::new(DiagnosticInner {
                events: Mutex::new(VecDeque::with_capacity(capacity.min(16_384))),
                capacity: capacity.max(1),
                log_dir,
                file: Mutex::new(FileState {
                    date: None,
                    file: None,
                    pending_lines: 0,
                    last_flush: Instant::now(),
                    error,
                }),
                redactor: Redactor::default(),
                debug_enabled: AtomicBool::new(false),
            }),
        }
    }

    pub fn redactor(&self) -> Redactor {
        self.inner.redactor.clone()
    }

    pub fn register_secret(&self, secret: &SecretString) {
        self.inner.redactor.register(secret);
    }

    pub fn log_directory(&self) -> &Path {
        &self.inner.log_dir
    }

    pub fn initialization_error(&self) -> Option<String> {
        self.inner
            .file
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .error
            .clone()
    }

    pub fn set_debug_enabled(&self, enabled: bool) {
        self.inner.debug_enabled.store(enabled, Ordering::Relaxed);
    }

    pub fn debug_enabled(&self) -> bool {
        self.inner.debug_enabled.load(Ordering::Relaxed)
    }

    pub fn record(
        &self,
        level: DiagnosticLevel,
        module: impl AsRef<str>,
        message: impl AsRef<str>,
        fields: impl IntoIterator<Item = (String, String)>,
    ) {
        if !self.level_enabled(level) {
            return;
        }
        let event = self.redact_event(DiagnosticEvent {
            timestamp: current_timestamp(),
            level,
            module: module.as_ref().to_owned(),
            message: message.as_ref().to_owned(),
            fields: fields.into_iter().collect(),
        });
        self.write_event(event);
    }

    pub fn recent(&self, filter: &DiagnosticFilter) -> Vec<DiagnosticEvent> {
        let module = filter.module.to_lowercase();
        let text = filter.text.to_lowercase();
        self.inner
            .events
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .iter()
            .filter(|event| filter.level.is_none_or(|level| level == event.level))
            .filter(|event| module.is_empty() || event.module.to_lowercase().contains(&module))
            .filter(|event| {
                text.is_empty()
                    || event.message.to_lowercase().contains(&text)
                    || event.fields.iter().any(|(key, value)| {
                        key.to_lowercase().contains(&text) || value.to_lowercase().contains(&text)
                    })
            })
            .cloned()
            .collect()
    }

    pub fn flush(&self) -> io::Result<()> {
        let mut state = self
            .inner
            .file
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(file) = state.file.as_mut() {
            file.flush()?;
        }
        state.pending_lines = 0;
        state.last_flush = Instant::now();
        Ok(())
    }

    pub fn clear(&self) -> io::Result<()> {
        self.inner
            .events
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clear();
        let mut state = self
            .inner
            .file
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.file = None;
        state.date = None;
        state.pending_lines = 0;
        state.last_flush = Instant::now();
        let mut first_error = None;
        if let Ok(entries) = fs::read_dir(&self.inner.log_dir) {
            for path in entries.filter_map(Result::ok).map(|entry| entry.path()) {
                if is_diagnostic_log(&path)
                    && let Err(error) = fs::remove_file(&path)
                    && first_error.is_none()
                {
                    first_error = Some(error);
                }
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    pub fn export_support_bundle(
        &self,
        destination: &Path,
        context: SupportBundleContext,
        profiles: &[ConnectionProfile],
    ) -> io::Result<()> {
        for profile in profiles {
            for identifier in [&profile.id, &profile.name, &profile.host, &profile.username] {
                self.inner.redactor.register_text(identifier);
            }
            if let remcmd_core::AuthConfig::PrivateKey { path } = &profile.auth {
                self.inner.redactor.register_text(&path.to_string_lossy());
                if let Ok(relative) = path.strip_prefix("~")
                    && let Some(base_dirs) = BaseDirs::new()
                {
                    self.inner
                        .redactor
                        .register_text(&base_dirs.home_dir().join(relative).to_string_lossy());
                }
            }
            if let Some(proxy) = profile.route.upstream_proxy.as_ref() {
                match proxy {
                    remcmd_core::ProxyConfig::HttpConnect { host, username, .. }
                    | remcmd_core::ProxyConfig::Socks5 { host, username, .. } => {
                        self.inner.redactor.register_text(host);
                        if let Some(username) = username {
                            self.inner.redactor.register_text(username);
                        }
                    }
                    remcmd_core::ProxyConfig::ProxyCommand { .. } => {}
                }
            }
            if let Some(remcmd_core::ProfileSource::OpenSsh {
                root_path, alias, ..
            }) = profile.source.as_ref()
            {
                self.inner
                    .redactor
                    .register_text(&root_path.to_string_lossy());
                self.inner.redactor.register_text(alias);
            }
        }
        self.flush()?;
        let parent = destination
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        {
            let mut archive = zip::ZipWriter::new(temporary.as_file_mut());
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);

            let manifest = SupportBundleManifest {
                generated_at: current_timestamp(),
                context,
                profiles: profiles.iter().map(anonymous_profile).collect(),
            };
            archive
                .start_file("support.json", options)
                .map_err(io::Error::other)?;
            archive.write_all(&serde_json::to_vec_pretty(&manifest).map_err(io::Error::other)?)?;

            let memory_events = self
                .inner
                .events
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .clone();
            archive
                .start_file("logs/memory.jsonl", options)
                .map_err(io::Error::other)?;
            for event in memory_events {
                let event = self.redact_event(event);
                let line = serde_json::to_string(&event).map_err(io::Error::other)?;
                archive.write_all(line.as_bytes())?;
                archive.write_all(b"\n")?;
            }

            let mut written_names = HashSet::new();
            if let Ok(entries) = fs::read_dir(&self.inner.log_dir) {
                let mut paths = entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.path())
                    .filter(|path| is_diagnostic_log(path))
                    .collect::<Vec<_>>();
                paths.sort();
                for path in paths {
                    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
                        continue;
                    };
                    if !written_names.insert(file_name.to_owned()) {
                        continue;
                    }
                    archive
                        .start_file(format!("logs/{file_name}"), options)
                        .map_err(io::Error::other)?;
                    let file = File::open(&path)?;
                    for line in BufReader::new(file).lines() {
                        let line = line?;
                        let Ok(event) = serde_json::from_str::<DiagnosticEvent>(&line) else {
                            continue;
                        };
                        let event = self.redact_event(event);
                        archive.write_all(
                            serde_json::to_string(&event)
                                .map_err(io::Error::other)?
                                .as_bytes(),
                        )?;
                        archive.write_all(b"\n")?;
                    }
                }
            }
            archive.finish().map_err(io::Error::other)?;
        }
        temporary.as_file().sync_all()?;
        temporary
            .persist(destination)
            .map_err(|error| error.error)?;
        Ok(())
    }

    fn level_enabled(&self, level: DiagnosticLevel) -> bool {
        level.priority() <= DiagnosticLevel::Info.priority()
            || self.inner.debug_enabled.load(Ordering::Relaxed)
    }

    fn redact_event(&self, mut event: DiagnosticEvent) -> DiagnosticEvent {
        let redactor = &self.inner.redactor;
        event.module = redactor.redact(&event.module);
        event.message = redactor.redact(&event.message);
        event.fields = event
            .fields
            .into_iter()
            .map(|(key, value)| {
                let value = if SENSITIVE_FIELD_NAME.is_match(&key) {
                    REDACTED.to_owned()
                } else {
                    redactor.redact(&value)
                };
                (redactor.redact(&key), value)
            })
            .collect();
        event
    }

    fn write_event(&self, event: DiagnosticEvent) {
        {
            let mut events = self
                .inner
                .events
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            while events.len() >= self.inner.capacity {
                events.pop_front();
            }
            events.push_back(event.clone());
        }

        let line = match serde_json::to_string(&event) {
            Ok(line) => line,
            Err(error) => {
                self.set_file_error(format!("Failed to encode a diagnostic event: {error}"));
                return;
            }
        };
        if let Err(error) = self.append_line(&line) {
            self.set_file_error(format!("Failed to write diagnostic logs: {error}"));
        }
    }

    fn append_line(&self, line: &str) -> io::Result<()> {
        let today = Utc::now().date_naive();
        let mut state = self
            .inner
            .file
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if state.date != Some(today) || state.file.is_none() {
            if let Some(file) = state.file.as_mut() {
                file.flush()?;
            }
            create_private_log_directory(&self.inner.log_dir)?;
            cleanup_old_logs(&self.inner.log_dir, today)?;
            let path = self.inner.log_dir.join(format!("remcmd-{today}.jsonl"));
            state.file = Some(BufWriter::with_capacity(
                LOG_BUFFER_CAPACITY,
                open_private_log_file(&path)?,
            ));
            state.date = Some(today);
            state.pending_lines = 0;
            state.last_flush = Instant::now();
            state.error = None;
        }
        {
            let file = state.file.as_mut().expect("daily log file was opened");
            file.write_all(line.as_bytes())?;
            file.write_all(b"\n")?;
        }
        state.pending_lines += 1;
        if state.pending_lines >= LOG_FLUSH_EVENT_INTERVAL
            || state.last_flush.elapsed() >= LOG_FLUSH_INTERVAL
        {
            state
                .file
                .as_mut()
                .expect("daily log file was opened")
                .flush()?;
            state.pending_lines = 0;
            state.last_flush = Instant::now();
        }
        Ok(())
    }

    fn set_file_error(&self, message: String) {
        let mut state = self
            .inner
            .file
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.file = None;
        state.pending_lines = 0;
        state.error = Some(message);
    }
}

#[derive(Clone)]
struct DiagnosticLayer {
    store: DiagnosticStore,
}

impl<S> Layer<S> for DiagnosticLayer
where
    S: Subscriber,
{
    fn enabled(&self, metadata: &Metadata<'_>, _context: Context<'_, S>) -> bool {
        self.store
            .level_enabled(DiagnosticLevel::from_tracing(metadata.level()))
    }

    fn on_event(&self, event: &Event<'_>, _context: Context<'_, S>) {
        let metadata = event.metadata();
        let mut visitor = EventVisitor::default();
        event.record(&mut visitor);
        self.store.record(
            DiagnosticLevel::from_tracing(metadata.level()),
            metadata.target(),
            visitor.message.as_deref().unwrap_or(metadata.name()),
            visitor.fields,
        );
    }
}

#[derive(Default)]
struct EventVisitor {
    message: Option<String>,
    fields: BTreeMap<String, String>,
}

impl Visit for EventVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn fmt::Debug) {
        let value = format!("{value:?}");
        if field.name() == "message" {
            self.message = Some(value.trim_matches('"').to_owned());
        } else {
            self.fields.insert(field.name().to_owned(), value);
        }
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.message = Some(value.to_owned());
        } else {
            self.fields
                .insert(field.name().to_owned(), value.to_owned());
        }
    }
}

pub struct Diagnostics {
    store: DiagnosticStore,
    subscriber_installed: bool,
}

impl Diagnostics {
    pub fn initialize(log_dir: impl Into<PathBuf>) -> Self {
        let store = DiagnosticStore::new(log_dir);
        let subscriber = tracing_subscriber::registry().with(DiagnosticLayer {
            store: store.clone(),
        });
        let subscriber_installed = tracing::subscriber::set_global_default(subscriber).is_ok();
        if !subscriber_installed {
            store.record(
                DiagnosticLevel::Warn,
                "diagnostics",
                "A global tracing subscriber was already installed; direct diagnostic capture remains available",
                [],
            );
        }
        Self {
            store,
            subscriber_installed,
        }
    }

    pub fn store(&self) -> DiagnosticStore {
        self.store.clone()
    }

    pub fn subscriber_installed(&self) -> bool {
        self.subscriber_installed
    }
}

pub fn default_log_directory() -> io::Result<PathBuf> {
    let project_dirs = ProjectDirs::from("", "", "RemCmd")
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "app data directory not found"))?;
    Ok(project_dirs.data_dir().join("logs"))
}

pub fn fallback_log_directory() -> PathBuf {
    let started_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = FALLBACK_DIRECTORY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "remcmd-diagnostics-{}-{started_at:x}-{sequence:x}",
        process::id(),
    ))
}

fn create_private_log_directory(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn open_private_log_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(file)
}

fn anonymous_profile(profile: &ConnectionProfile) -> AnonymousProfile {
    AnonymousProfile {
        id_hash: short_hash(profile.id.as_bytes()),
        endpoint_hash: short_hash(format!("{}:{}", profile.host, profile.port).as_bytes()),
        uses_default_ssh_port: profile.port == 22,
        authentication: match profile.auth {
            remcmd_core::AuthConfig::Password => "password",
            remcmd_core::AuthConfig::None => "none",
            remcmd_core::AuthConfig::PrivateKey { .. } => "private_key",
            remcmd_core::AuthConfig::Agent => "agent",
        },
        proxy: match profile.route.upstream_proxy {
            None => "none",
            Some(remcmd_core::ProxyConfig::HttpConnect { .. }) => "http_connect",
            Some(remcmd_core::ProxyConfig::Socks5 { .. }) => "socks5",
            Some(remcmd_core::ProxyConfig::ProxyCommand { .. }) => "proxy_command",
        },
        jump_count: profile.route.jump_host_ids.len(),
        imported_from_openssh: profile.source.is_some(),
    }
}

fn short_hash(value: &[u8]) -> String {
    let digest = Sha256::digest(value);
    digest[..12]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn current_timestamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn cleanup_old_logs(log_dir: &Path, today: NaiveDate) -> io::Result<()> {
    let cutoff = today - Duration::days(LOG_RETENTION_DAYS - 1);
    for entry in fs::read_dir(log_dir)? {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some(date) = name
            .strip_prefix("remcmd-")
            .and_then(|name| name.strip_suffix(".jsonl"))
            .and_then(|date| NaiveDate::parse_from_str(date, "%Y-%m-%d").ok())
        else {
            continue;
        };
        if date < cutoff {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

fn is_diagnostic_log(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("remcmd-") && name.ends_with(".jsonl"))
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use super::*;

    #[test]
    fn redactor_covers_registered_and_structured_secret_patterns() {
        let redactor = Redactor::default();
        redactor.register_text("literal-canary");
        let input = "literal-canary https://alice:hunter2@example.test Authorization: Bearer123 password=hunter2 token: abc";

        let output = redactor.redact(input);

        assert!(!output.contains("literal-canary"));
        assert!(!output.contains("alice:hunter2"));
        assert!(!output.contains("Bearer123"));
        assert!(!output.contains("hunter2"));
        assert!(!output.contains("abc"));
        assert!(output.matches(REDACTED).count() >= 5);
    }

    #[test]
    fn file_failure_falls_back_to_filterable_memory_events() {
        let directory = tempfile::tempdir().unwrap();
        let not_a_directory = directory.path().join("logs");
        fs::write(&not_a_directory, "occupied").unwrap();
        let store = DiagnosticStore::new(&not_a_directory);

        store.record(
            DiagnosticLevel::Info,
            "ssh::transport",
            "connection failed",
            [("stage".into(), "target".into())],
        );

        assert!(store.initialization_error().is_some());
        assert_eq!(
            store
                .recent(&DiagnosticFilter {
                    level: Some(DiagnosticLevel::Info),
                    module: "ssh".into(),
                    text: "target".into(),
                })
                .len(),
            1
        );
    }

    #[test]
    fn debug_events_are_temporary_and_disabled_by_default() {
        let directory = tempfile::tempdir().unwrap();
        let store = DiagnosticStore::new(directory.path());
        store.record(DiagnosticLevel::Debug, "test", "hidden", []);
        assert!(store.recent(&DiagnosticFilter::default()).is_empty());

        store.set_debug_enabled(true);
        store.record(DiagnosticLevel::Debug, "test", "visible", []);
        assert_eq!(store.recent(&DiagnosticFilter::default()).len(), 1);
    }

    #[test]
    fn retention_keeps_today_and_previous_six_days() {
        let directory = tempfile::tempdir().unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 8, 9).unwrap();
        fs::write(directory.path().join("remcmd-2026-08-03.jsonl"), "keep").unwrap();
        fs::write(directory.path().join("remcmd-2026-08-02.jsonl"), "delete").unwrap();
        fs::write(directory.path().join("unrelated.txt"), "keep").unwrap();

        cleanup_old_logs(directory.path(), today).unwrap();

        assert!(directory.path().join("remcmd-2026-08-03.jsonl").exists());
        assert!(!directory.path().join("remcmd-2026-08-02.jsonl").exists());
        assert!(directory.path().join("unrelated.txt").exists());
    }

    #[test]
    fn fallback_log_directories_are_unique_and_private() {
        let first = fallback_log_directory();
        let second = fallback_log_directory();
        assert_ne!(first, second);

        let store = DiagnosticStore::new(&first);
        assert_eq!(store.log_directory(), first);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&first).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700);
        }

        drop(store);
        fs::remove_dir_all(first).unwrap();
    }

    #[test]
    fn buffered_logs_can_be_flushed_explicitly() {
        let directory = tempfile::tempdir().unwrap();
        let store = DiagnosticStore::new(directory.path());
        store.record(DiagnosticLevel::Info, "test", "buffered event", []);

        store.flush().unwrap();

        let contents = fs::read_dir(directory.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| fs::read_to_string(entry.path()).unwrap())
            .collect::<String>();
        assert!(contents.contains("buffered event"));
    }

    #[test]
    fn support_bundle_redacts_disk_memory_and_excludes_sensitive_profile_fields() {
        let directory = tempfile::tempdir().unwrap();
        let store = DiagnosticStore::new(directory.path().join("logs"));
        store.redactor().register_text("literal-canary");
        store.record(
            DiagnosticLevel::Error,
            "ssh",
            "literal-canary password=hunter2 failed to load /Users/alice/.ssh/id_company",
            [
                ("uri".into(), "ssh://alice:hunter2@example.test".into()),
                ("proxy_password".into(), "field-canary".into()),
                ("Authorization".into(), "Bearer field-token".into()),
            ],
        );
        let forbidden_event_values = [
            "literal-canary",
            "hunter2",
            "alice:hunter2",
            "field-canary",
            "field-token",
        ];
        let memory = serde_json::to_string(&store.recent(&DiagnosticFilter::default())).unwrap();
        store.flush().unwrap();
        let disk = fs::read_dir(directory.path().join("logs"))
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| fs::read_to_string(entry.path()).unwrap())
            .collect::<String>();
        for forbidden in forbidden_event_values {
            assert!(!memory.contains(forbidden), "found {forbidden} in memory");
            assert!(!disk.contains(forbidden), "found {forbidden} on disk");
        }
        let mut profile = ConnectionProfile::new(
            "sensitive-id",
            "Private Production Name",
            "secret.example.test",
            2222,
            "alice",
        );
        profile.auth = remcmd_core::AuthConfig::PrivateKey {
            path: PathBuf::from("/Users/alice/.ssh/id_company"),
        };
        profile.route.upstream_proxy = Some(remcmd_core::ProxyConfig::ProxyCommand {
            command_digest: "safe-digest".into(),
            approved_digest: Some("safe-approval".into()),
        });
        let destination = directory.path().join("support.zip");

        store
            .export_support_bundle(
                &destination,
                SupportBundleContext::for_current_platform("test"),
                &[profile],
            )
            .unwrap();

        let file = File::open(destination).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let mut contents = String::new();
        for index in 0..archive.len() {
            let mut file = archive.by_index(index).unwrap();
            file.read_to_string(&mut contents).unwrap();
        }
        for forbidden in [
            "literal-canary",
            "hunter2",
            "alice:hunter2",
            "field-canary",
            "field-token",
            "sensitive-id",
            "Private Production Name",
            "secret.example.test",
            "alice",
            "2222",
            "/Users/alice/.ssh/id_company",
        ] {
            assert!(!contents.contains(forbidden), "found {forbidden} in bundle");
        }
        assert!(contents.contains("proxy_command"));
        assert!(contents.contains(REDACTED));
    }
}
