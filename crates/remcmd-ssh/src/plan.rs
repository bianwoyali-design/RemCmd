use std::collections::HashSet;

use remcmd_core::ConnectionProfile;
use secrecy::{ExposeSecret, SecretString};
use sha2::{Digest, Sha256};

use crate::{AuthMethod, AuthMethodKind, SshError, SshErrorKind};

/// Identifies the current connection stage without carrying credentials.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectionStage {
    Proxy,
    Jump {
        index: usize,
        total: usize,
        profile_id: String,
    },
    Target {
        profile_id: String,
    },
}

impl ConnectionStage {
    pub fn profile_id(&self) -> Option<&str> {
        match self {
            Self::Proxy => None,
            Self::Jump { profile_id, .. } | Self::Target { profile_id } => Some(profile_id),
        }
    }
}

/// One SSH endpoint and its independent authentication material.
///
/// This type intentionally does not implement Debug, Clone, Serialize, or
/// Deserialize because it owns runtime credentials.
pub struct ConnectionStep {
    pub(crate) profile: ConnectionProfile,
    pub(crate) auth: AuthMethod,
}

impl ConnectionStep {
    pub fn new(profile: ConnectionProfile, auth: AuthMethod) -> Self {
        Self { profile, auth }
    }

    pub fn profile(&self) -> &ConnectionProfile {
        &self.profile
    }

    pub fn authentication_kind(&self) -> AuthMethodKind {
        self.auth.kind()
    }
}

/// Runtime proxy configuration, including secrets fetched from the keychain.
///
/// This type intentionally does not implement Debug, Clone, Serialize, or
/// Deserialize.
pub enum RuntimeProxy {
    HttpConnect {
        host: String,
        port: u16,
        username: Option<String>,
        password: Option<SecretString>,
    },
    Socks5 {
        host: String,
        port: u16,
        username: Option<String>,
        password: Option<SecretString>,
    },
    ProxyCommand {
        command: SecretString,
        approved_digest: Option<String>,
    },
}

impl RuntimeProxy {
    pub fn http_connect(
        host: impl Into<String>,
        port: u16,
        username: Option<String>,
        password: Option<SecretString>,
    ) -> Self {
        Self::HttpConnect {
            host: host.into(),
            port,
            username,
            password,
        }
    }

    pub fn socks5(
        host: impl Into<String>,
        port: u16,
        username: Option<String>,
        password: Option<SecretString>,
    ) -> Self {
        Self::Socks5 {
            host: host.into(),
            port,
            username,
            password,
        }
    }

    pub fn proxy_command(command: SecretString, approved_digest: Option<String>) -> Self {
        Self::ProxyCommand {
            command,
            approved_digest,
        }
    }

    pub(crate) fn is_proxy_command(&self) -> bool {
        matches!(self, Self::ProxyCommand { .. })
    }
}

/// Expanded ProxyCommand text and the digest that approves exactly this use.
///
/// The command remains a SecretString and this type deliberately has no Debug
/// or Clone implementation.
pub struct ProxyCommandPreview {
    expanded_command: SecretString,
    approval_digest: String,
    approved: bool,
}

impl ProxyCommandPreview {
    pub fn expanded_command(&self) -> &SecretString {
        &self.expanded_command
    }

    pub fn approval_digest(&self) -> &str {
        &self.approval_digest
    }

    pub fn is_approved(&self) -> bool {
        self.approved
    }
}

/// Complete runtime route to one target.
///
/// This type intentionally does not implement Debug, Clone, Serialize, or
/// Deserialize so secrets cannot be duplicated or formatted accidentally.
pub struct ConnectionPlan {
    pub(crate) target: ConnectionStep,
    pub(crate) jumps: Vec<ConnectionStep>,
    pub(crate) proxy: Option<RuntimeProxy>,
}

impl ConnectionPlan {
    pub fn direct(profile: ConnectionProfile, auth: AuthMethod) -> Self {
        Self {
            target: ConnectionStep::new(profile, auth),
            jumps: Vec::new(),
            proxy: None,
        }
    }

    pub fn new(target: ConnectionStep) -> Self {
        Self {
            target,
            jumps: Vec::new(),
            proxy: None,
        }
    }

    pub fn set_proxy(&mut self, proxy: RuntimeProxy) {
        self.proxy = Some(proxy);
    }

    pub fn push_jump(&mut self, jump: ConnectionStep) {
        self.jumps.push(jump);
    }

    pub fn target_profile(&self) -> &ConnectionProfile {
        self.target.profile()
    }

    pub fn jump_profiles(&self) -> impl ExactSizeIterator<Item = &ConnectionProfile> {
        self.jumps.iter().map(ConnectionStep::profile)
    }

    pub fn has_proxy(&self) -> bool {
        self.proxy.is_some()
    }

    pub fn validate(&self) -> Result<(), SshError> {
        if self
            .proxy
            .as_ref()
            .is_some_and(RuntimeProxy::is_proxy_command)
            && !self.jumps.is_empty()
        {
            return Err(SshError::new(
                SshErrorKind::Configuration,
                "ProxyCommand and jump hosts cannot be used together",
            ));
        }
        let target_id = &self.target.profile.id;
        let mut jump_ids = HashSet::new();
        for jump in &self.jumps {
            if &jump.profile.id == target_id {
                return Err(SshError::new(
                    SshErrorKind::Configuration,
                    "a connection cannot use itself as a jump host",
                ));
            }
            if !jump_ids.insert(&jump.profile.id) {
                return Err(SshError::new(
                    SshErrorKind::Configuration,
                    "a jump host cannot appear more than once",
                ));
            }
        }
        for step in self.jumps.iter().chain(std::iter::once(&self.target)) {
            if step.profile.host.trim().is_empty()
                || step.profile.username.trim().is_empty()
                || step.profile.port == 0
            {
                return Err(SshError::new(
                    SshErrorKind::Configuration,
                    "every connection step requires a host, port, and username",
                ));
            }
        }
        if let Some(proxy) = self.proxy.as_ref() {
            match proxy {
                RuntimeProxy::HttpConnect {
                    host,
                    port,
                    username,
                    password,
                }
                | RuntimeProxy::Socks5 {
                    host,
                    port,
                    username,
                    password,
                } => {
                    let credentials_are_valid = match (username, password) {
                        (None, None) => true,
                        (Some(username), Some(_)) => !username.trim().is_empty(),
                        _ => false,
                    };
                    if host.trim().is_empty() || *port == 0 || !credentials_are_valid {
                        return Err(SshError::new(
                            SshErrorKind::Configuration,
                            "proxy host, port, username, and password configuration is incomplete",
                        ));
                    }
                }
                RuntimeProxy::ProxyCommand { command, .. }
                    if command.expose_secret().trim().is_empty() =>
                {
                    return Err(SshError::new(
                        SshErrorKind::Configuration,
                        "ProxyCommand cannot be empty",
                    ));
                }
                RuntimeProxy::ProxyCommand { .. } => {}
            }
        }
        Ok(())
    }

    pub fn proxy_command_preview(&self) -> Result<Option<ProxyCommandPreview>, SshError> {
        let Some(RuntimeProxy::ProxyCommand {
            command,
            approved_digest,
        }) = self.proxy.as_ref()
        else {
            return Ok(None);
        };
        let first = self.jumps.first().unwrap_or(&self.target);
        let expanded = expand_proxy_command(command.expose_secret(), &first.profile)?;
        let approval_digest =
            proxy_command_approval_digest(command.expose_secret(), &first.profile);
        Ok(Some(ProxyCommandPreview {
            expanded_command: SecretString::new(expanded.into_boxed_str()),
            approved: approved_digest.as_deref() == Some(approval_digest.as_str()),
            approval_digest,
        }))
    }

    pub fn approve_proxy_command(&mut self, approval_digest: String) -> Result<(), SshError> {
        let preview = self.proxy_command_preview()?.ok_or_else(|| {
            SshError::new(
                SshErrorKind::Configuration,
                "the connection plan does not contain a ProxyCommand",
            )
        })?;
        if preview.approval_digest() != approval_digest {
            return Err(SshError::new(
                SshErrorKind::ProxyCommandApproval,
                "ProxyCommand approval no longer matches the current target parameters",
            ));
        }
        let Some(RuntimeProxy::ProxyCommand {
            approved_digest, ..
        }) = self.proxy.as_mut()
        else {
            unreachable!("preview verified the ProxyCommand variant")
        };
        *approved_digest = Some(approval_digest);
        Ok(())
    }

    pub(crate) fn into_parts(self) -> (ConnectionStep, Vec<ConnectionStep>, Option<RuntimeProxy>) {
        (self.target, self.jumps, self.proxy)
    }
}

pub(crate) fn expand_proxy_command(
    command: &str,
    endpoint: &ConnectionProfile,
) -> Result<String, SshError> {
    let mut output = String::with_capacity(command.len());
    let mut characters = command.chars();
    while let Some(character) = characters.next() {
        if character != '%' {
            output.push(character);
            continue;
        }
        match characters.next() {
            Some('%') => output.push('%'),
            Some('h') => output.push_str(&endpoint.host),
            Some('n') => output.push_str(&endpoint.name),
            Some('p') => output.push_str(&endpoint.port.to_string()),
            Some('r') => output.push_str(&endpoint.username),
            Some(token) => {
                return Err(SshError::new(
                    SshErrorKind::Configuration,
                    format!("unsupported ProxyCommand token %{token}"),
                ));
            }
            None => {
                return Err(SshError::new(
                    SshErrorKind::Configuration,
                    "ProxyCommand ends with an incomplete % token",
                ));
            }
        }
    }
    Ok(output)
}

pub(crate) fn proxy_command_approval_digest(command: &str, endpoint: &ConnectionProfile) -> String {
    let mut hasher = Sha256::new();
    for value in [
        command,
        endpoint.name.as_str(),
        endpoint.host.as_str(),
        &endpoint.port.to_string(),
        endpoint.username.as_str(),
    ] {
        hasher.update(value.as_bytes());
        hasher.update([0]);
    }
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn proxy_command_content_digest(command: &str) -> String {
    Sha256::digest(command.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::ExposeSecret;

    fn profile(id: &str) -> ConnectionProfile {
        ConnectionProfile::new(id, id, format!("{id}.example"), 22, "alice")
    }

    #[test]
    fn connection_plan_rejects_recursive_and_duplicate_jumps() {
        let mut recursive = ConnectionPlan::direct(profile("target"), AuthMethod::None);
        recursive.push_jump(ConnectionStep::new(profile("target"), AuthMethod::None));
        assert!(recursive.validate().is_err());

        let mut duplicate = ConnectionPlan::direct(profile("target"), AuthMethod::None);
        duplicate.push_jump(ConnectionStep::new(profile("jump"), AuthMethod::None));
        duplicate.push_jump(ConnectionStep::new(profile("jump"), AuthMethod::None));
        assert!(duplicate.validate().is_err());
    }

    #[test]
    fn proxy_command_and_jump_hosts_are_mutually_exclusive() {
        let mut plan = ConnectionPlan::direct(profile("target"), AuthMethod::None);
        plan.push_jump(ConnectionStep::new(profile("jump"), AuthMethod::None));
        plan.set_proxy(RuntimeProxy::proxy_command(
            SecretString::new("ssh bridge -W %h:%p".into()),
            None,
        ));
        assert!(plan.validate().is_err());
    }

    #[test]
    fn proxy_command_expands_supported_tokens_and_digest_tracks_target_identity() {
        let mut target = profile("alias");
        target.host = "internal.example".into();
        target.port = 2222;
        let command = "bridge %% %n %h %p %r";
        let mut plan = ConnectionPlan::direct(target.clone(), AuthMethod::None);
        plan.set_proxy(RuntimeProxy::proxy_command(
            SecretString::new(command.into()),
            None,
        ));

        let preview = plan.proxy_command_preview().unwrap().unwrap();
        assert_eq!(
            preview.expanded_command().expose_secret(),
            "bridge % alias internal.example 2222 alice"
        );
        let first_digest = preview.approval_digest().to_owned();
        target.port = 2223;
        assert_ne!(
            first_digest,
            proxy_command_approval_digest(command, &target)
        );
    }

    #[test]
    fn proxy_command_approval_accepts_only_the_current_preview_digest() {
        let mut plan = ConnectionPlan::direct(profile("target"), AuthMethod::None);
        plan.set_proxy(RuntimeProxy::proxy_command(
            SecretString::new("bridge %h %p".into()),
            None,
        ));

        let digest = plan
            .proxy_command_preview()
            .unwrap()
            .unwrap()
            .approval_digest()
            .to_owned();
        assert!(plan.approve_proxy_command("stale".into()).is_err());
        assert!(!plan.proxy_command_preview().unwrap().unwrap().is_approved());

        plan.approve_proxy_command(digest).unwrap();
        assert!(plan.proxy_command_preview().unwrap().unwrap().is_approved());
    }

    #[test]
    fn connection_plan_rejects_incomplete_proxy_authentication() {
        let mut plan = ConnectionPlan::direct(profile("target"), AuthMethod::None);
        plan.set_proxy(RuntimeProxy::http_connect(
            "proxy.example",
            8080,
            Some("alice".into()),
            None,
        ));
        assert!(plan.validate().is_err());

        let mut plan = ConnectionPlan::direct(profile("target"), AuthMethod::None);
        plan.set_proxy(RuntimeProxy::socks5(
            "proxy.example",
            1080,
            None,
            Some(SecretString::new("password".into())),
        ));
        assert!(plan.validate().is_err());
    }
}
