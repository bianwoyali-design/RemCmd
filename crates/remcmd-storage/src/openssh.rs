use std::{
    collections::{BTreeSet, HashMap, HashSet},
    env, fmt, fs, io,
    path::{Path, PathBuf},
};

use directories::BaseDirs;
use glob::glob;
use remcmd_core::{AuthConfig, ConnectionProfile, ProfileSource, ProxyConfig};
use secrecy::SecretString;
use sha2::{Digest, Sha256};
use wildmatch::WildMatch;

use crate::{
    credentials::{CredentialBackend, SystemCredentialBackend, delete_with, load_with, save_with},
    save_profiles,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenSshImportStatus {
    New,
    Update,
    Unchanged,
    Conflict,
    Invalid,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenSshImportWarning {
    pub path: PathBuf,
    pub line: usize,
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct OpenSshImportCandidate {
    pub alias: String,
    pub profile: Option<ConnectionProfile>,
    pub status: OpenSshImportStatus,
    pub warnings: Vec<OpenSshImportWarning>,
    proxy_command: Option<SecretString>,
    identity_file: Option<PathBuf>,
}

impl OpenSshImportCandidate {
    pub fn proxy_command(&self) -> Option<&SecretString> {
        self.proxy_command.as_ref()
    }

    pub fn take_proxy_command(&mut self) -> Option<SecretString> {
        self.proxy_command.take()
    }

    pub fn identity_file(&self) -> Option<&Path> {
        self.identity_file.as_deref()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenSshApplyError {
    message: String,
}

impl OpenSshApplyError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for OpenSshApplyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for OpenSshApplyError {}

#[derive(Clone, Debug)]
pub struct OpenSshImportPreview {
    pub root_path: PathBuf,
    pub candidates: Vec<OpenSshImportCandidate>,
    pub warnings: Vec<OpenSshImportWarning>,
}

pub fn default_openssh_config_path() -> io::Result<PathBuf> {
    let base_dirs = BaseDirs::new()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "home directory not found"))?;
    Ok(base_dirs.home_dir().join(".ssh/config"))
}

pub fn preview_openssh_import(
    path: &Path,
    existing_profiles: &[ConnectionProfile],
) -> io::Result<OpenSshImportPreview> {
    let base_dirs = BaseDirs::new()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "home directory not found"))?;
    let local_user = env::var("USER")
        .or_else(|_| env::var("USERNAME"))
        .unwrap_or_else(|_| "unknown".into());
    preview_with_context(path, existing_profiles, base_dirs.home_dir(), &local_user)
}

/// Applies selected import candidates as one profiles/keychain transaction.
///
/// ProxyJump dependencies are included automatically. Conflicts keep the local
/// profile unless the alias is also present in `overwrite_conflicts`.
pub fn apply_openssh_import(
    profiles_path: &Path,
    existing_profiles: &[ConnectionProfile],
    candidates: &[OpenSshImportCandidate],
    selected_aliases: &HashSet<String>,
    overwrite_conflicts: &HashSet<String>,
) -> Result<Vec<ConnectionProfile>, OpenSshApplyError> {
    apply_import_with(
        &SystemCredentialBackend,
        profiles_path,
        existing_profiles,
        candidates,
        selected_aliases,
        overwrite_conflicts,
    )
}

fn apply_import_with(
    backend: &impl CredentialBackend,
    profiles_path: &Path,
    existing_profiles: &[ConnectionProfile],
    candidates: &[OpenSshImportCandidate],
    selected_aliases: &HashSet<String>,
    overwrite_conflicts: &HashSet<String>,
) -> Result<Vec<ConnectionProfile>, OpenSshApplyError> {
    let candidate_by_alias: HashMap<&str, &OpenSshImportCandidate> = candidates
        .iter()
        .map(|candidate| (candidate.alias.as_str(), candidate))
        .collect();
    let candidate_by_id: HashMap<&str, &OpenSshImportCandidate> = candidates
        .iter()
        .filter_map(|candidate| {
            candidate
                .profile
                .as_ref()
                .map(|profile| (profile.id.as_str(), candidate))
        })
        .collect();

    let mut selected = BTreeSet::new();
    for alias in selected_aliases {
        let candidate = candidate_by_alias
            .get(alias.as_str())
            .ok_or_else(|| OpenSshApplyError::new(format!("Unknown import candidate {alias}")))?;
        if candidate.status == OpenSshImportStatus::Invalid {
            return Err(OpenSshApplyError::new(format!(
                "Cannot import invalid OpenSSH host {alias}"
            )));
        }
        selected.insert(alias.clone());
    }

    loop {
        let mut added_dependency = false;
        for alias in selected.clone() {
            let candidate = candidate_by_alias[alias.as_str()];
            if !candidate_will_replace(candidate, overwrite_conflicts) {
                continue;
            }
            let Some(profile) = candidate.profile.as_ref() else {
                continue;
            };
            for jump_id in &profile.route.jump_host_ids {
                if existing_profiles
                    .iter()
                    .any(|profile| profile.id == *jump_id)
                {
                    continue;
                }
                let dependency = candidate_by_id.get(jump_id.as_str()).ok_or_else(|| {
                    OpenSshApplyError::new(format!(
                        "OpenSSH host {alias} references an unavailable jump host"
                    ))
                })?;
                if dependency.status == OpenSshImportStatus::Invalid {
                    return Err(OpenSshApplyError::new(format!(
                        "OpenSSH host {alias} references an invalid jump host"
                    )));
                }
                added_dependency |= selected.insert(dependency.alias.clone());
            }
        }
        if !added_dependency {
            break;
        }
    }

    let mut next_profiles = existing_profiles.to_vec();
    let mut replacements = Vec::new();
    for alias in selected {
        let candidate = candidate_by_alias[alias.as_str()];
        if !candidate_will_replace(candidate, overwrite_conflicts) {
            continue;
        }
        let profile = candidate
            .profile
            .as_ref()
            .expect("valid import candidates contain a profile")
            .clone();
        if let Some(index) = next_profiles
            .iter()
            .position(|existing| existing.id == profile.id)
        {
            next_profiles[index] = profile.clone();
        } else {
            next_profiles.push(profile.clone());
        }
        replacements.push((candidate, profile));
    }

    for profile in &next_profiles {
        let mut seen = HashSet::new();
        for jump_id in &profile.route.jump_host_ids {
            if jump_id == &profile.id {
                return Err(OpenSshApplyError::new(format!(
                    "Profile {} cannot use itself as a jump host",
                    profile.name
                )));
            }
            if !seen.insert(jump_id) {
                return Err(OpenSshApplyError::new(format!(
                    "Profile {} contains a duplicate jump host",
                    profile.name
                )));
            }
            if !next_profiles.iter().any(|jump| jump.id == *jump_id) {
                return Err(OpenSshApplyError::new(format!(
                    "Profile {} references a missing jump host",
                    profile.name
                )));
            }
        }
    }

    let mut credential_changes = Vec::new();
    for (candidate, profile) in &replacements {
        let previous = existing_profiles
            .iter()
            .find(|existing| existing.id == profile.id);
        let was_proxy_command = previous.is_some_and(|profile| {
            matches!(
                profile.route.upstream_proxy,
                Some(ProxyConfig::ProxyCommand { .. })
            )
        });
        let is_proxy_command = matches!(
            profile.route.upstream_proxy,
            Some(ProxyConfig::ProxyCommand { .. })
        );
        if !was_proxy_command && !is_proxy_command {
            continue;
        }

        let before = load_with(backend, &profile.id, crate::CredentialKind::ProxyCommand)
            .map_err(|error| OpenSshApplyError::new(error.to_string()))?;
        let result = if is_proxy_command {
            let command = candidate.proxy_command.as_ref().ok_or_else(|| {
                OpenSshApplyError::new(format!(
                    "ProxyCommand for {} is no longer available in memory",
                    candidate.alias
                ))
            })?;
            save_with(
                backend,
                &profile.id,
                crate::CredentialKind::ProxyCommand,
                command,
            )
        } else {
            delete_with(backend, &profile.id, crate::CredentialKind::ProxyCommand)
        };
        if let Err(error) = result {
            rollback_credentials(backend, &credential_changes);
            return Err(OpenSshApplyError::new(error.to_string()));
        }
        credential_changes.push((profile.id.clone(), before));
    }

    if let Err(error) = save_profiles(profiles_path, &next_profiles) {
        rollback_credentials(backend, &credential_changes);
        return Err(OpenSshApplyError::new(format!(
            "Failed to atomically save imported profiles: {error}"
        )));
    }

    Ok(next_profiles)
}

fn candidate_will_replace(
    candidate: &OpenSshImportCandidate,
    overwrite_conflicts: &HashSet<String>,
) -> bool {
    match candidate.status {
        OpenSshImportStatus::New | OpenSshImportStatus::Update => true,
        OpenSshImportStatus::Conflict => overwrite_conflicts.contains(&candidate.alias),
        OpenSshImportStatus::Unchanged | OpenSshImportStatus::Invalid => false,
    }
}

fn rollback_credentials(
    backend: &impl CredentialBackend,
    changes: &[(String, Option<SecretString>)],
) {
    for (profile_id, previous) in changes.iter().rev() {
        if let Some(previous) = previous {
            let _ = save_with(
                backend,
                profile_id,
                crate::CredentialKind::ProxyCommand,
                previous,
            );
        } else {
            let _ = delete_with(backend, profile_id, crate::CredentialKind::ProxyCommand);
        }
    }
}

fn preview_with_context(
    path: &Path,
    existing_profiles: &[ConnectionProfile],
    home_dir: &Path,
    local_user: &str,
) -> io::Result<OpenSshImportPreview> {
    let root_path = fs::canonicalize(path)?;
    let mut parser = ConfigParser::new(home_dir.to_path_buf(), local_user.to_owned());
    parser.parse_root(&root_path)?;

    let mut used_ids: HashSet<String> = existing_profiles
        .iter()
        .map(|profile| profile.id.clone())
        .collect();
    let existing_by_source = existing_profiles_by_source(existing_profiles, &root_path);
    let mut effective_by_alias = HashMap::new();
    for alias in &parser.aliases {
        effective_by_alias.insert(
            alias.clone(),
            resolve_host(alias, &parser.rules, home_dir, local_user),
        );
    }

    let mut id_by_alias = HashMap::new();
    for alias in &parser.aliases {
        let source_key = alias.as_str();
        let id = existing_by_source
            .get(source_key)
            .map(|profile| profile.id.clone())
            .unwrap_or_else(|| generated_profile_id(&root_path, source_key, &mut used_ids));
        id_by_alias.insert(alias.clone(), id);
    }

    let mut synthetic = HashMap::<String, (String, EffectiveHost)>::new();
    for (alias, effective) in &effective_by_alias {
        let Some(RouteDirective::Jump(specification)) = effective.route.as_ref() else {
            continue;
        };
        let expanded = expand_connection_tokens(specification, alias, effective);
        for jump in split_proxy_jump(&expanded) {
            if jump == "none" || is_plain_alias(&jump, &id_by_alias) {
                continue;
            }
            let source_key = format!("@jump:{jump}");
            if synthetic.contains_key(&source_key) {
                continue;
            }
            let Some(destination) = parse_jump_destination(&jump) else {
                continue;
            };
            let base = effective_by_alias
                .get(&destination.host)
                .cloned()
                .unwrap_or_else(|| EffectiveHost::for_alias(&jump, local_user));
            let mut resolved = base;
            resolved.host_name = Some(destination.host);
            if let Some(username) = destination.username {
                resolved.user = Some(username);
            }
            if let Some(port) = destination.port {
                resolved.port = Some(port.to_string());
            }
            resolved.route = None;
            let id = existing_by_source
                .get(source_key.as_str())
                .map(|profile| profile.id.clone())
                .unwrap_or_else(|| generated_profile_id(&root_path, &source_key, &mut used_ids));
            synthetic.insert(source_key, (id, resolved));
        }
    }

    let mut jump_id_by_spec = HashMap::new();
    for (source_key, (id, _)) in &synthetic {
        jump_id_by_spec.insert(
            source_key.trim_start_matches("@jump:").to_owned(),
            id.clone(),
        );
    }

    let candidate_context = CandidateContext {
        root_path: &root_path,
        id_by_alias: &id_by_alias,
        jump_id_by_spec: &jump_id_by_spec,
    };
    let mut candidates = Vec::new();
    for alias in &parser.aliases {
        let effective = effective_by_alias
            .get(alias)
            .expect("every parsed alias has effective configuration");
        let id = id_by_alias
            .get(alias)
            .expect("every parsed alias has a generated id")
            .clone();
        candidates.push(build_candidate(
            alias,
            alias,
            id,
            effective,
            &candidate_context,
            existing_by_source.get(alias.as_str()).copied(),
        ));
    }
    for (source_key, (id, effective)) in synthetic {
        let display_alias = source_key.trim_start_matches("@jump:");
        candidates.push(build_candidate(
            display_alias,
            &source_key,
            id,
            &effective,
            &candidate_context,
            existing_by_source.get(source_key.as_str()).copied(),
        ));
    }
    candidates.sort_by(|left, right| left.alias.cmp(&right.alias));

    Ok(OpenSshImportPreview {
        root_path,
        candidates,
        warnings: parser.warnings,
    })
}

fn existing_profiles_by_source<'a>(
    profiles: &'a [ConnectionProfile],
    root_path: &Path,
) -> HashMap<&'a str, &'a ConnectionProfile> {
    profiles
        .iter()
        .filter_map(|profile| match profile.source.as_ref()? {
            ProfileSource::OpenSsh {
                root_path: source_root,
                alias,
                ..
            } if source_root == root_path => Some((alias.as_str(), profile)),
            _ => None,
        })
        .collect()
}

struct CandidateContext<'a> {
    root_path: &'a Path,
    id_by_alias: &'a HashMap<String, String>,
    jump_id_by_spec: &'a HashMap<String, String>,
}

fn build_candidate(
    display_alias: &str,
    source_alias: &str,
    id: String,
    effective: &EffectiveHost,
    context: &CandidateContext<'_>,
    existing: Option<&ConnectionProfile>,
) -> OpenSshImportCandidate {
    let mut warnings = effective.warnings.clone();
    if effective.proxy_use_fdpass {
        warnings.push(OpenSshImportWarning {
            path: effective.location.path.clone(),
            line: effective.location.line,
            message: "ProxyUseFdpass is not supported".into(),
        });
        return invalid_candidate(display_alias, warnings);
    }

    let host = effective
        .host_name
        .as_deref()
        .unwrap_or(display_alias)
        .to_owned();
    let username = effective.user.clone().unwrap_or_else(|| "unknown".into());
    let port = match effective.port.as_deref().unwrap_or("22").parse::<u16>() {
        Ok(port) if port > 0 => port,
        _ => {
            warnings.push(OpenSshImportWarning {
                path: effective.location.path.clone(),
                line: effective.location.line,
                message: "Port must be a number from 1 to 65535".into(),
            });
            return invalid_candidate(display_alias, warnings);
        }
    };

    let mut profile = ConnectionProfile::new(id, display_alias, host, port, username);
    let identity_file = effective.identity_files.first().cloned();
    if let Some(identity) = identity_file.as_ref() {
        profile.auth = AuthConfig::PrivateKey {
            path: identity.clone(),
        };
        if effective.identity_files.len() > 1 {
            warnings.push(OpenSshImportWarning {
                path: effective.location.path.clone(),
                line: effective.location.line,
                message: format!(
                    "Only the first of {} IdentityFile values will be imported",
                    effective.identity_files.len()
                ),
            });
        }
    } else if cfg!(windows) {
        profile.auth = AuthConfig::Password;
    } else {
        profile.auth = AuthConfig::Agent;
    }

    let mut proxy_command = None;
    match effective.route.as_ref() {
        Some(RouteDirective::Jump(specification)) if specification != "none" => {
            let expanded = expand_connection_tokens(specification, display_alias, effective);
            let jumps = split_proxy_jump(&expanded);
            let mut route_invalid = jumps.is_empty();
            if route_invalid {
                warnings.push(OpenSshImportWarning {
                    path: effective.location.path.clone(),
                    line: effective.location.line,
                    message: "ProxyJump does not contain a usable destination".into(),
                });
            }
            for jump in jumps {
                if jump == "none" {
                    continue;
                }
                if let Some(id) = context.id_by_alias.get(&jump) {
                    profile.route.jump_host_ids.push(id.clone());
                } else if let Some(id) = context.jump_id_by_spec.get(&jump) {
                    profile.route.jump_host_ids.push(id.clone());
                } else {
                    warnings.push(OpenSshImportWarning {
                        path: effective.location.path.clone(),
                        line: effective.location.line,
                        message: format!("Could not resolve ProxyJump entry {jump}"),
                    });
                    route_invalid = true;
                }
            }
            let mut seen = HashSet::new();
            if profile
                .route
                .jump_host_ids
                .iter()
                .any(|jump_id| jump_id == &profile.id || !seen.insert(jump_id))
            {
                warnings.push(OpenSshImportWarning {
                    path: effective.location.path.clone(),
                    line: effective.location.line,
                    message: "ProxyJump cannot reference itself or repeat a destination".into(),
                });
                route_invalid = true;
            }
            if route_invalid {
                return invalid_candidate(display_alias, warnings);
            }
        }
        Some(RouteDirective::Command(command)) if command != "none" => {
            if let Err(message) = validate_proxy_command_tokens(command) {
                warnings.push(OpenSshImportWarning {
                    path: effective.location.path.clone(),
                    line: effective.location.line,
                    message,
                });
                return invalid_candidate(display_alias, warnings);
            }
            let command_digest = digest_bytes(command.as_bytes());
            profile.route.upstream_proxy = Some(ProxyConfig::ProxyCommand {
                command_digest,
                approved_digest: None,
            });
            proxy_command = Some(SecretString::new(command.clone().into_boxed_str()));
        }
        Some(RouteDirective::Jump(_) | RouteDirective::Command(_)) | None => {}
    }

    let import_digest = profile_import_digest(&profile);
    profile.source = Some(ProfileSource::OpenSsh {
        root_path: context.root_path.to_path_buf(),
        alias: source_alias.to_owned(),
        last_import_digest: import_digest.clone(),
    });

    let status = match existing {
        None => OpenSshImportStatus::New,
        Some(existing) => {
            profile.id.clone_from(&existing.id);
            let previous_digest = match existing.source.as_ref() {
                Some(ProfileSource::OpenSsh {
                    last_import_digest, ..
                }) => last_import_digest,
                None => unreachable!("existing source map only contains imported profiles"),
            };
            let current_digest = profile_import_digest(existing);
            if &import_digest == previous_digest || current_digest == import_digest {
                OpenSshImportStatus::Unchanged
            } else if &current_digest == previous_digest {
                OpenSshImportStatus::Update
            } else {
                OpenSshImportStatus::Conflict
            }
        }
    };

    OpenSshImportCandidate {
        alias: display_alias.to_owned(),
        profile: Some(profile),
        status,
        warnings,
        proxy_command,
        identity_file,
    }
}

fn invalid_candidate(alias: &str, warnings: Vec<OpenSshImportWarning>) -> OpenSshImportCandidate {
    OpenSshImportCandidate {
        alias: alias.to_owned(),
        profile: None,
        status: OpenSshImportStatus::Invalid,
        warnings,
        proxy_command: None,
        identity_file: None,
    }
}

pub fn profile_import_digest(profile: &ConnectionProfile) -> String {
    let mut normalized = profile.clone();
    normalized.id.clear();
    normalized.source = None;
    if let Some(ProxyConfig::ProxyCommand {
        approved_digest, ..
    }) = normalized.route.upstream_proxy.as_mut()
    {
        *approved_digest = None;
    }
    let bytes =
        serde_json::to_vec(&normalized).expect("connection profile serialization is infallible");
    digest_bytes(&bytes)
}

fn generated_profile_id(root_path: &Path, alias: &str, used_ids: &mut HashSet<String>) -> String {
    let digest = digest_bytes(format!("{}\0{alias}", root_path.display()).as_bytes());
    let base = format!("openssh-{}", &digest[..16]);
    let mut id = base.clone();
    let mut suffix = 2;
    while !used_ids.insert(id.clone()) {
        id = format!("{base}-{suffix}");
        suffix += 1;
    }
    id
}

fn digest_bytes(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}

#[derive(Clone, Debug)]
struct EffectiveHost {
    host_name: Option<String>,
    user: Option<String>,
    user_was_set: bool,
    port: Option<String>,
    identity_files: Vec<PathBuf>,
    route: Option<RouteDirective>,
    proxy_use_fdpass: bool,
    location: SourceLocation,
    warnings: Vec<OpenSshImportWarning>,
}

impl EffectiveHost {
    fn for_alias(_alias: &str, local_user: &str) -> Self {
        Self {
            host_name: None,
            user: Some(local_user.to_owned()),
            user_was_set: false,
            port: None,
            identity_files: Vec::new(),
            route: None,
            proxy_use_fdpass: false,
            location: SourceLocation {
                path: PathBuf::new(),
                line: 0,
            },
            warnings: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
enum RouteDirective {
    Jump(String),
    Command(String),
}

#[derive(Clone, Debug)]
struct Rule {
    scope: Scope,
    keyword: String,
    arguments: String,
    location: SourceLocation,
}

#[derive(Clone, Debug)]
enum Scope {
    All,
    Host(Vec<String>),
    Match(Vec<MatchTerm>),
    Unsupported,
}

#[derive(Clone, Debug)]
enum MatchTerm {
    All,
    Host(Vec<String>),
    OriginalHost(Vec<String>),
    User(Vec<String>),
    LocalUser(Vec<String>),
}

#[derive(Clone, Debug)]
struct SourceLocation {
    path: PathBuf,
    line: usize,
}

struct ConfigParser {
    home_dir: PathBuf,
    ssh_dir: PathBuf,
    local_user: String,
    rules: Vec<Rule>,
    aliases: BTreeSet<String>,
    warnings: Vec<OpenSshImportWarning>,
    scope: Scope,
    include_stack: Vec<PathBuf>,
}

impl ConfigParser {
    fn new(home_dir: PathBuf, local_user: String) -> Self {
        Self {
            ssh_dir: home_dir.join(".ssh"),
            home_dir,
            local_user,
            rules: Vec::new(),
            aliases: BTreeSet::new(),
            warnings: Vec::new(),
            scope: Scope::All,
            include_stack: Vec::new(),
        }
    }

    fn parse_root(&mut self, path: &Path) -> io::Result<()> {
        self.parse_file(path, true)
    }

    fn parse_file(&mut self, path: &Path, required: bool) -> io::Result<()> {
        let canonical = match fs::canonicalize(path) {
            Ok(path) => path,
            Err(error) if !required && error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        if self.include_stack.contains(&canonical) {
            self.warnings.push(OpenSshImportWarning {
                path: canonical,
                line: 0,
                message: "Include cycle ignored".into(),
            });
            return Ok(());
        }
        let content = fs::read_to_string(&canonical)?;
        self.include_stack.push(canonical.clone());

        for (index, raw_line) in content.lines().enumerate() {
            let line_number = index + 1;
            let Some((keyword, arguments)) = parse_config_line(raw_line) else {
                continue;
            };
            let keyword_lower = keyword.to_ascii_lowercase();
            let location = SourceLocation {
                path: canonical.clone(),
                line: line_number,
            };
            match keyword_lower.as_str() {
                "include" => self.parse_includes(&arguments, &location),
                "host" => self.set_host_scope(&arguments),
                "match" => self.set_match_scope(&arguments, &location),
                _ => self.rules.push(Rule {
                    scope: self.scope.clone(),
                    keyword: keyword_lower,
                    arguments,
                    location,
                }),
            }
        }

        self.include_stack.pop();
        Ok(())
    }

    fn parse_includes(&mut self, arguments: &str, location: &SourceLocation) {
        for include in split_words(arguments) {
            let expanded = match expand_global_tokens(&include, &self.home_dir, &self.local_user) {
                Ok(expanded) => expanded,
                Err(message) => {
                    self.warnings.push(OpenSshImportWarning {
                        path: location.path.clone(),
                        line: location.line,
                        message,
                    });
                    continue;
                }
            };
            let path = PathBuf::from(expanded);
            let pattern = if path.is_absolute() {
                path
            } else {
                self.ssh_dir.join(path)
            };
            let pattern_text = pattern.to_string_lossy().into_owned();
            let mut matches = match glob(&pattern_text) {
                Ok(paths) => paths.filter_map(Result::ok).collect::<Vec<_>>(),
                Err(error) => {
                    self.warnings.push(OpenSshImportWarning {
                        path: location.path.clone(),
                        line: location.line,
                        message: format!("Invalid Include pattern: {error}"),
                    });
                    continue;
                }
            };
            matches.sort();
            for path in matches {
                if let Err(error) = self.parse_file(&path, false) {
                    self.warnings.push(OpenSshImportWarning {
                        path: location.path.clone(),
                        line: location.line,
                        message: format!("Failed to read Include {}: {error}", path.display()),
                    });
                }
            }
        }
    }

    fn set_host_scope(&mut self, arguments: &str) {
        let patterns = split_pattern_list(arguments);
        for pattern in &patterns {
            if !pattern.starts_with('!') && !pattern.contains(['*', '?']) {
                self.aliases.insert(pattern.clone());
            }
        }
        self.scope = Scope::Host(patterns);
    }

    fn set_match_scope(&mut self, arguments: &str, location: &SourceLocation) {
        match parse_match_terms(arguments) {
            Ok(terms) => self.scope = Scope::Match(terms),
            Err(message) => {
                self.scope = Scope::Unsupported;
                self.warnings.push(OpenSshImportWarning {
                    path: location.path.clone(),
                    line: location.line,
                    message,
                });
            }
        }
    }
}

fn resolve_host(alias: &str, rules: &[Rule], home_dir: &Path, local_user: &str) -> EffectiveHost {
    let mut effective = EffectiveHost::for_alias(alias, local_user);
    let mut identity_files = Vec::new();
    for rule in rules {
        if !scope_matches(&rule.scope, alias, &effective, local_user) {
            continue;
        }
        if effective.location.path.as_os_str().is_empty() {
            effective.location = rule.location.clone();
        }
        match rule.keyword.as_str() {
            "hostname" if effective.host_name.is_none() => {
                if let Some(value) = split_words(&rule.arguments).first() {
                    effective.host_name = Some(expand_host_name_tokens(value, alias));
                }
            }
            "user" if !effective.user_was_set => {
                if let Some(value) = split_words(&rule.arguments).first() {
                    effective.user = Some(value.clone());
                    effective.user_was_set = true;
                }
            }
            "port" if effective.port.is_none() => {
                effective.port = split_words(&rule.arguments).first().cloned();
            }
            "identityfile" => {
                if let Some(value) = split_words(&rule.arguments).first() {
                    identity_files.push((value.clone(), rule.location.clone()));
                }
            }
            "proxyjump" if effective.route.is_none() => {
                effective.route = Some(RouteDirective::Jump(rule.arguments.trim().to_owned()));
            }
            "proxycommand" if effective.route.is_none() => {
                effective.route = Some(RouteDirective::Command(rule.arguments.trim().to_owned()));
            }
            "proxyusefdpass" => {
                effective.proxy_use_fdpass |= split_words(&rule.arguments)
                    .first()
                    .is_some_and(|value| value.eq_ignore_ascii_case("yes"));
            }
            _ => {}
        }
    }
    for (value, location) in identity_files {
        match expand_identity_path(&value, alias, &effective, home_dir, local_user) {
            Ok(path) if path != Path::new("none") => effective.identity_files.push(path),
            Ok(_) => {}
            Err(message) => effective.warnings.push(OpenSshImportWarning {
                path: location.path,
                line: location.line,
                message,
            }),
        }
    }
    effective
}

fn scope_matches(scope: &Scope, alias: &str, effective: &EffectiveHost, local_user: &str) -> bool {
    match scope {
        Scope::All => true,
        Scope::Host(patterns) => pattern_list_matches(patterns, alias),
        Scope::Unsupported => false,
        Scope::Match(terms) => terms.iter().all(|term| match term {
            MatchTerm::All => true,
            MatchTerm::Host(patterns) => {
                pattern_list_matches(patterns, effective.host_name.as_deref().unwrap_or(alias))
            }
            MatchTerm::OriginalHost(patterns) => pattern_list_matches(patterns, alias),
            MatchTerm::User(patterns) => {
                pattern_list_matches(patterns, effective.user.as_deref().unwrap_or(local_user))
            }
            MatchTerm::LocalUser(patterns) => pattern_list_matches(patterns, local_user),
        }),
    }
}

fn pattern_list_matches(patterns: &[String], value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    let mut positive = false;
    for pattern in patterns {
        let (negated, pattern) = pattern
            .strip_prefix('!')
            .map_or((false, pattern.as_str()), |pattern| (true, pattern));
        if WildMatch::new(&pattern.to_ascii_lowercase()).matches(&value) {
            if negated {
                return false;
            }
            positive = true;
        }
    }
    positive
}

fn parse_match_terms(arguments: &str) -> Result<Vec<MatchTerm>, String> {
    let words = split_words(arguments);
    if words.len() == 1 && words[0].eq_ignore_ascii_case("all") {
        return Ok(vec![MatchTerm::All]);
    }
    let mut terms = Vec::new();
    let mut index = 0;
    while index < words.len() {
        let keyword = words[index].to_ascii_lowercase();
        if keyword == "exec" {
            return Err("Match exec was skipped because imports never execute commands".into());
        }
        let Some(value) = words.get(index + 1) else {
            return Err(format!("Match {keyword} is missing a value"));
        };
        let patterns = split_pattern_list(value);
        let term = match keyword.as_str() {
            "host" => MatchTerm::Host(patterns),
            "originalhost" => MatchTerm::OriginalHost(patterns),
            "user" => MatchTerm::User(patterns),
            "localuser" => MatchTerm::LocalUser(patterns),
            _ => return Err(format!("Unsupported Match condition {keyword} was skipped")),
        };
        terms.push(term);
        index += 2;
    }
    Ok(terms)
}

fn parse_config_line(line: &str) -> Option<(String, String)> {
    let mut escaped = false;
    let mut quote = None;
    let mut end = line.len();
    for (index, character) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if character == '\\' {
            escaped = true;
            continue;
        }
        if let Some(active_quote) = quote {
            if character == active_quote {
                quote = None;
            }
            continue;
        }
        if character == '"' || character == '\'' {
            quote = Some(character);
        } else if character == '#' {
            end = index;
            break;
        }
    }
    let line = line[..end].trim();
    if line.is_empty() {
        return None;
    }
    let keyword_end = line
        .find(|character: char| character.is_ascii_whitespace() || character == '=')
        .unwrap_or(line.len());
    let keyword = line[..keyword_end].trim();
    let arguments = line[keyword_end..]
        .trim_start_matches(|character: char| character.is_ascii_whitespace() || character == '=')
        .trim()
        .to_owned();
    (!keyword.is_empty()).then(|| (keyword.to_owned(), arguments))
}

fn split_words(value: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut escaped = false;
    for character in value.chars() {
        if escaped {
            current.push(character);
            escaped = false;
            continue;
        }
        if character == '\\' {
            escaped = true;
            continue;
        }
        if let Some(active_quote) = quote {
            if character == active_quote {
                quote = None;
            } else {
                current.push(character);
            }
            continue;
        }
        if character == '"' || character == '\'' {
            quote = Some(character);
        } else if character.is_ascii_whitespace() {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
        } else {
            current.push(character);
        }
    }
    if escaped {
        current.push('\\');
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

fn split_pattern_list(value: &str) -> Vec<String> {
    split_words(value)
        .into_iter()
        .flat_map(|word| {
            word.split(',')
                .filter(|pattern| !pattern.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .collect()
}

fn expand_global_tokens(value: &str, home_dir: &Path, local_user: &str) -> Result<String, String> {
    let value = expand_environment(value)?;
    let mut output = String::new();
    let mut characters = value.chars();
    while let Some(character) = characters.next() {
        if character != '%' {
            output.push(character);
            continue;
        }
        match characters.next() {
            Some('%') => output.push('%'),
            Some('d') => output.push_str(&home_dir.to_string_lossy()),
            Some('u') => output.push_str(local_user),
            Some(token) => {
                return Err(format!(
                    "Include token %{token} depends on a connection and was skipped"
                ));
            }
            None => return Err("Include ends with an incomplete % token".into()),
        }
    }
    Ok(expand_tilde(&output, home_dir)
        .to_string_lossy()
        .into_owned())
}

fn expand_identity_path(
    value: &str,
    alias: &str,
    effective: &EffectiveHost,
    home_dir: &Path,
    local_user: &str,
) -> Result<PathBuf, String> {
    let value = expand_environment(value)?;
    let host = effective.host_name.as_deref().unwrap_or(alias);
    let port = effective.port.as_deref().unwrap_or("22");
    let remote_user = effective.user.as_deref().unwrap_or(local_user);
    let mut output = String::new();
    let mut characters = value.chars();
    while let Some(character) = characters.next() {
        if character != '%' {
            output.push(character);
            continue;
        }
        match characters.next() {
            Some('%') => output.push('%'),
            Some('d') => output.push_str(&home_dir.to_string_lossy()),
            Some('h') => output.push_str(host),
            Some('n') => output.push_str(alias),
            Some('p') => output.push_str(port),
            Some('r') => output.push_str(remote_user),
            Some('u') => output.push_str(local_user),
            Some(token) => return Err(format!("Unsupported IdentityFile token %{token}")),
            None => return Err("IdentityFile ends with an incomplete % token".into()),
        }
    }
    Ok(expand_tilde(&output, home_dir))
}

fn expand_environment(value: &str) -> Result<String, String> {
    let mut output = String::new();
    let mut rest = value;
    while let Some(start) = rest.find("${") {
        output.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            return Err("Unclosed environment variable expression".into());
        };
        let name = &after[..end];
        let expanded =
            env::var(name).map_err(|_| format!("Environment variable {name} is not available"))?;
        output.push_str(&expanded);
        rest = &after[end + 1..];
    }
    output.push_str(rest);
    Ok(output)
}

fn expand_tilde(value: &str, home_dir: &Path) -> PathBuf {
    let path = Path::new(value);
    match path.strip_prefix("~") {
        Ok(relative) => home_dir.join(relative),
        Err(_) => path.to_path_buf(),
    }
}

fn expand_connection_tokens(value: &str, alias: &str, effective: &EffectiveHost) -> String {
    let host = effective.host_name.as_deref().unwrap_or(alias);
    let port = effective.port.as_deref().unwrap_or("22");
    let user = effective.user.as_deref().unwrap_or("unknown");
    let mut output = String::new();
    let mut characters = value.chars();
    while let Some(character) = characters.next() {
        if character != '%' {
            output.push(character);
            continue;
        }
        match characters.next() {
            Some('%') => output.push('%'),
            Some('h') => output.push_str(host),
            Some('n') => output.push_str(alias),
            Some('p') => output.push_str(port),
            Some('r') => output.push_str(user),
            Some(token) => {
                output.push('%');
                output.push(token);
            }
            None => output.push('%'),
        }
    }
    output
}

fn expand_host_name_tokens(value: &str, alias: &str) -> String {
    let mut output = String::new();
    let mut characters = value.chars();
    while let Some(character) = characters.next() {
        if character != '%' {
            output.push(character);
            continue;
        }
        match characters.next() {
            Some('%') => output.push('%'),
            Some('h') => output.push_str(alias),
            Some(token) => {
                output.push('%');
                output.push(token);
            }
            None => output.push('%'),
        }
    }
    output
}

fn validate_proxy_command_tokens(command: &str) -> Result<(), String> {
    let mut characters = command.chars();
    while let Some(character) = characters.next() {
        if character != '%' {
            continue;
        }
        match characters.next() {
            Some('%' | 'h' | 'n' | 'p' | 'r') => {}
            Some(token) => {
                return Err(format!(
                    "ProxyCommand contains unsupported token %{token}; supported tokens are %% %h %n %p %r"
                ));
            }
            None => return Err("ProxyCommand ends with an incomplete % token".into()),
        }
    }
    Ok(())
}

fn split_proxy_jump(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect()
}

fn is_plain_alias(value: &str, aliases: &HashMap<String, String>) -> bool {
    aliases.contains_key(value)
}

#[derive(Debug)]
struct JumpDestination {
    username: Option<String>,
    host: String,
    port: Option<u16>,
}

fn parse_jump_destination(value: &str) -> Option<JumpDestination> {
    let value = value.strip_prefix("ssh://").unwrap_or(value);
    let (username, host_port) = value
        .rsplit_once('@')
        .map_or((None, value), |(username, host)| {
            (Some(username.to_owned()), host)
        });
    if username.as_deref().is_some_and(str::is_empty) {
        return None;
    }
    let (host, port) = if let Some(rest) = host_port.strip_prefix('[') {
        let (host, rest) = rest.split_once(']')?;
        let port = if rest.is_empty() {
            None
        } else {
            Some(
                rest.strip_prefix(':')?
                    .parse::<u16>()
                    .ok()
                    .filter(|port| *port > 0)?,
            )
        };
        (host.to_owned(), port)
    } else if host_port.matches(':').count() == 1 {
        let (host, port) = host_port.rsplit_once(':')?;
        (
            host.to_owned(),
            Some(port.parse::<u16>().ok().filter(|port| *port > 0)?),
        )
    } else {
        (host_port.to_owned(), None)
    };
    (!host.trim().is_empty()).then_some(JumpDestination {
        username,
        host,
        port,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use secrecy::ExposeSecret;

    #[derive(Default)]
    struct MemoryCredentialBackend {
        entries: Mutex<HashMap<String, String>>,
        fail_saves: bool,
    }

    impl CredentialBackend for MemoryCredentialBackend {
        fn load(&self, account: &str) -> Result<Option<String>, crate::CredentialStoreError> {
            Ok(self.entries.lock().unwrap().get(account).cloned())
        }

        fn save(&self, account: &str, secret: &str) -> Result<(), crate::CredentialStoreError> {
            if self.fail_saves {
                return Err(crate::CredentialStoreError::new(
                    "save",
                    "test backend failure",
                ));
            }
            self.entries
                .lock()
                .unwrap()
                .insert(account.to_owned(), secret.to_owned());
            Ok(())
        }

        fn delete(&self, account: &str) -> Result<(), crate::CredentialStoreError> {
            self.entries.lock().unwrap().remove(account);
            Ok(())
        }
    }

    fn write_config(directory: &Path, text: &str) -> PathBuf {
        let ssh = directory.join(".ssh");
        fs::create_dir_all(&ssh).unwrap();
        let path = ssh.join("config");
        fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn imports_literal_hosts_with_defaults_tokens_and_includes() {
        let directory = tempfile::tempdir().unwrap();
        let ssh = directory.path().join(".ssh");
        fs::create_dir_all(ssh.join("config.d")).unwrap();
        fs::write(
            ssh.join("config.d/work"),
            "Host work\n  HostName work.example.com\n  IdentityFile %d/.ssh/id_%r\n",
        )
        .unwrap();
        let path = write_config(
            directory.path(),
            "Include config.d/*\nHost *\n  User alice\n  Port 2222\nHost *.internal\n  User ignored\n",
        );

        let preview = preview_with_context(&path, &[], directory.path(), "local").unwrap();

        assert_eq!(preview.candidates.len(), 1);
        let profile = preview.candidates[0].profile.as_ref().unwrap();
        assert_eq!(profile.name, "work");
        assert_eq!(profile.host, "work.example.com");
        assert_eq!(profile.username, "alice");
        assert_eq!(profile.port, 2222);
        assert_eq!(
            profile.auth,
            AuthConfig::PrivateKey {
                path: directory.path().join(".ssh/id_alice")
            }
        );
    }

    #[test]
    fn host_negation_and_first_value_win_match_openssh_rules() {
        let directory = tempfile::tempdir().unwrap();
        let path = write_config(
            directory.path(),
            "Host !blocked.example.com *.example.com\n  User wildcard\nHost good.example.com\n  User specific\nHost blocked.example.com\n  User blocked\n",
        );

        let preview = preview_with_context(&path, &[], directory.path(), "local").unwrap();
        let good = preview
            .candidates
            .iter()
            .find(|candidate| candidate.alias == "good.example.com")
            .unwrap()
            .profile
            .as_ref()
            .unwrap();
        let blocked = preview
            .candidates
            .iter()
            .find(|candidate| candidate.alias == "blocked.example.com")
            .unwrap()
            .profile
            .as_ref()
            .unwrap();

        assert_eq!(good.username, "wildcard");
        assert_eq!(blocked.username, "blocked");
    }

    #[test]
    fn match_exec_is_never_executed_and_is_reported() {
        let directory = tempfile::tempdir().unwrap();
        let marker = directory.path().join("must-not-exist");
        let path = write_config(
            directory.path(),
            &format!(
                "Host safe\n  HostName safe.example\nMatch exec \"touch {}\"\n  User attacker\n",
                marker.display()
            ),
        );

        let preview = preview_with_context(&path, &[], directory.path(), "local").unwrap();

        assert!(!marker.exists());
        assert!(
            preview
                .warnings
                .iter()
                .any(|warning| warning.message.contains("never execute"))
        );
        assert_eq!(
            preview.candidates[0].profile.as_ref().unwrap().username,
            "local"
        );
    }

    #[test]
    fn proxy_jump_creates_ordered_references_and_synthetic_hosts() {
        let directory = tempfile::tempdir().unwrap();
        let path = write_config(
            directory.path(),
            "Host bastion\n  HostName bastion.example\nHost target\n  HostName target.internal\n  ProxyJump bastion,ops@second.example:2200\n",
        );

        let preview = preview_with_context(&path, &[], directory.path(), "local").unwrap();
        let bastion = preview
            .candidates
            .iter()
            .find(|candidate| candidate.alias == "bastion")
            .unwrap()
            .profile
            .as_ref()
            .unwrap();
        let second = preview
            .candidates
            .iter()
            .find(|candidate| candidate.alias == "ops@second.example:2200")
            .unwrap()
            .profile
            .as_ref()
            .unwrap();
        let target = preview
            .candidates
            .iter()
            .find(|candidate| candidate.alias == "target")
            .unwrap()
            .profile
            .as_ref()
            .unwrap();

        assert_eq!(second.host, "second.example");
        assert_eq!(second.username, "ops");
        assert_eq!(second.port, 2200);
        assert_eq!(
            target.route.jump_host_ids,
            vec![bastion.id.clone(), second.id.clone()]
        );
    }

    #[test]
    fn unresolved_proxy_jump_is_an_invalid_candidate() {
        let directory = tempfile::tempdir().unwrap();
        let path = write_config(
            directory.path(),
            "Host target\n  ProxyJump ssh://missing.example:not-a-port\n",
        );

        let preview = preview_with_context(&path, &[], directory.path(), "local").unwrap();
        let target = preview
            .candidates
            .iter()
            .find(|candidate| candidate.alias == "target")
            .unwrap();

        assert_eq!(target.status, OpenSshImportStatus::Invalid);
        assert!(target.profile.is_none());
        assert!(
            target
                .warnings
                .iter()
                .any(|warning| warning.message.contains("Could not resolve ProxyJump"))
        );
        assert!(parse_jump_destination("@missing.example").is_none());
        assert!(parse_jump_destination("missing.example:0").is_none());
    }

    #[test]
    fn safe_match_subset_uses_resolved_and_original_connection_values() {
        let directory = tempfile::tempdir().unwrap();
        let path = write_config(
            directory.path(),
            "Host target\n  HostName internal.example\n  User alice\nMatch originalhost target\n  Port 2201\nMatch host internal.example user alice localuser local\n  IdentityFile %d/.ssh/id_match\n",
        );

        let preview = preview_with_context(&path, &[], directory.path(), "local").unwrap();
        let profile = preview.candidates[0].profile.as_ref().unwrap();

        assert_eq!(profile.port, 2201);
        assert_eq!(
            profile.auth,
            AuthConfig::PrivateKey {
                path: directory.path().join(".ssh/id_match")
            }
        );
    }

    #[test]
    fn multiple_identity_files_keep_the_first_and_warn() {
        let directory = tempfile::tempdir().unwrap();
        let path = write_config(
            directory.path(),
            "Host target\n  IdentityFile ~/.ssh/id_first\n  IdentityFile ~/.ssh/id_second\n",
        );

        let preview = preview_with_context(&path, &[], directory.path(), "local").unwrap();
        let candidate = &preview.candidates[0];

        assert_eq!(
            candidate.profile.as_ref().unwrap().auth,
            AuthConfig::PrivateKey {
                path: directory.path().join(".ssh/id_first")
            }
        );
        assert_eq!(
            candidate.identity_file(),
            Some(directory.path().join(".ssh/id_first").as_path())
        );
        assert!(
            candidate
                .warnings
                .iter()
                .any(|warning| warning.message.contains("Only the first"))
        );
        assert!(expand_environment("${PATH}/ssh").unwrap().ends_with("/ssh"));
    }

    #[test]
    fn proxy_command_is_kept_out_of_profile_json() {
        let directory = tempfile::tempdir().unwrap();
        let path = write_config(
            directory.path(),
            "Host proxied\n  ProxyCommand nc -X connect -x proxy:8080 %h %p\n",
        );

        let preview = preview_with_context(&path, &[], directory.path(), "local").unwrap();
        let candidate = &preview.candidates[0];
        let profile_json = serde_json::to_string(candidate.profile.as_ref().unwrap()).unwrap();

        assert_eq!(
            candidate.proxy_command().unwrap().expose_secret(),
            "nc -X connect -x proxy:8080 %h %p"
        );
        assert!(!profile_json.contains("nc -X"));
        assert!(matches!(
            candidate.profile.as_ref().unwrap().route.upstream_proxy,
            Some(ProxyConfig::ProxyCommand { .. })
        ));
    }

    #[test]
    fn host_name_token_expansion_preserves_escaped_percent_tokens() {
        let directory = tempfile::tempdir().unwrap();
        let path = write_config(
            directory.path(),
            "Host escaped\n  HostName %%h.%h.example\n",
        );

        let preview = preview_with_context(&path, &[], directory.path(), "local").unwrap();
        let profile = preview.candidates[0].profile.as_ref().unwrap();

        assert_eq!(profile.host, "%h.escaped.example");
    }

    #[test]
    fn proxy_command_invalid_tokens_are_reported_during_preview() {
        for (command, expected_warning) in [
            ("nc %x", "unsupported token %x"),
            ("nc %", "incomplete % token"),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let path = write_config(
                directory.path(),
                &format!("Host proxied\n  ProxyCommand {command}\n"),
            );

            let preview = preview_with_context(&path, &[], directory.path(), "local").unwrap();
            let candidate = &preview.candidates[0];

            assert_eq!(candidate.status, OpenSshImportStatus::Invalid);
            assert!(candidate.profile.is_none());
            assert!(
                candidate
                    .warnings
                    .iter()
                    .any(|warning| warning.message.contains(expected_warning))
            );
        }
    }

    #[test]
    fn reimport_distinguishes_update_conflict_and_unchanged() {
        let directory = tempfile::tempdir().unwrap();
        let path = write_config(directory.path(), "Host server\n  HostName old.example\n");
        let first = preview_with_context(&path, &[], directory.path(), "local").unwrap();
        let imported = first.candidates[0].profile.clone().unwrap();

        let unchanged = preview_with_context(
            &path,
            std::slice::from_ref(&imported),
            directory.path(),
            "local",
        )
        .unwrap();
        assert_eq!(
            unchanged.candidates[0].status,
            OpenSshImportStatus::Unchanged
        );

        fs::write(&path, "Host server\n  HostName new.example\n").unwrap();
        let update = preview_with_context(
            &path,
            std::slice::from_ref(&imported),
            directory.path(),
            "local",
        )
        .unwrap();
        assert_eq!(update.candidates[0].status, OpenSshImportStatus::Update);
        assert_eq!(
            update.candidates[0].profile.as_ref().unwrap().id,
            imported.id
        );

        let mut locally_edited = imported;
        locally_edited.name = "My Server".into();
        let conflict =
            preview_with_context(&path, &[locally_edited], directory.path(), "local").unwrap();
        assert_eq!(conflict.candidates[0].status, OpenSshImportStatus::Conflict);
    }

    #[test]
    fn include_cycles_are_reported_without_recursing_forever() {
        let directory = tempfile::tempdir().unwrap();
        let ssh = directory.path().join(".ssh");
        fs::create_dir_all(&ssh).unwrap();
        let path = ssh.join("config");
        fs::write(&path, "Include config\nHost server\n").unwrap();

        let preview = preview_with_context(&path, &[], directory.path(), "local").unwrap();

        assert_eq!(preview.candidates.len(), 1);
        assert!(
            preview
                .warnings
                .iter()
                .any(|warning| warning.message.contains("cycle"))
        );
    }

    #[test]
    fn parser_supports_equals_quotes_and_comments() {
        assert_eq!(
            parse_config_line("HostName = \"host name\" # comment"),
            Some(("HostName".into(), "\"host name\"".into()))
        );
        assert_eq!(
            split_words("\"host name\" plain"),
            vec!["host name", "plain"]
        );
    }

    #[test]
    fn applying_target_automatically_includes_new_jump_dependencies() {
        let directory = tempfile::tempdir().unwrap();
        let config = write_config(
            directory.path(),
            "Host jump\n  HostName jump.example\nHost target\n  HostName target.internal\n  ProxyJump jump\n",
        );
        let preview = preview_with_context(&config, &[], directory.path(), "local").unwrap();
        let profiles_path = directory.path().join("profiles.json");
        let selected = HashSet::from(["target".to_owned()]);

        let profiles = apply_import_with(
            &MemoryCredentialBackend::default(),
            &profiles_path,
            &[],
            &preview.candidates,
            &selected,
            &HashSet::new(),
        )
        .unwrap();

        assert_eq!(profiles.len(), 2);
        let target = profiles
            .iter()
            .find(|profile| profile.name == "target")
            .unwrap();
        let jump = profiles
            .iter()
            .find(|profile| profile.name == "jump")
            .unwrap();
        assert_eq!(
            target.route.jump_host_ids.as_slice(),
            std::slice::from_ref(&jump.id)
        );
        assert_eq!(crate::load_profiles(&profiles_path).unwrap(), profiles);
    }

    #[test]
    fn failed_profiles_write_rolls_back_proxy_command_secret() {
        let directory = tempfile::tempdir().unwrap();
        let config = write_config(
            directory.path(),
            "Host target\n  HostName target.internal\n  ProxyCommand ssh bridge -W %h:%p\n",
        );
        let preview = preview_with_context(&config, &[], directory.path(), "local").unwrap();
        let selected = HashSet::from(["target".to_owned()]);
        let backend = MemoryCredentialBackend::default();

        let error = apply_import_with(
            &backend,
            directory.path(),
            &[],
            &preview.candidates,
            &selected,
            &HashSet::new(),
        )
        .unwrap_err();

        assert!(error.to_string().contains("atomically save"));
        assert!(backend.entries.lock().unwrap().is_empty());
    }

    #[test]
    fn unavailable_keychain_rejects_proxy_command_without_writing_profiles() {
        let directory = tempfile::tempdir().unwrap();
        let config = write_config(
            directory.path(),
            "Host target\n  ProxyCommand ssh bridge -W %h:%p\n",
        );
        let preview = preview_with_context(&config, &[], directory.path(), "local").unwrap();
        let profiles_path = directory.path().join("profiles.json");
        let selected = HashSet::from(["target".to_owned()]);
        let backend = MemoryCredentialBackend {
            entries: Mutex::default(),
            fail_saves: true,
        };

        apply_import_with(
            &backend,
            &profiles_path,
            &[],
            &preview.candidates,
            &selected,
            &HashSet::new(),
        )
        .unwrap_err();

        assert!(!profiles_path.exists());
    }
}
