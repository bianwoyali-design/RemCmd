use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ConnectionProfile {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    #[serde(default)]
    pub auth: AuthConfig,
    #[serde(default)]
    pub route: ConnectionRoute,
    #[serde(default)]
    pub source: Option<ProfileSource>,
}

impl ConnectionProfile {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        host: impl Into<String>,
        port: u16,
        username: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            host: host.into(),
            port,
            username: username.into(),
            auth: AuthConfig::default(),
            route: ConnectionRoute::default(),
            source: None,
        }
    }

    pub fn samples() -> Vec<Self> {
        vec![
            Self::new("local-dev", "Local Dev", "127.0.0.1", 22, "dev"),
            Self::new("staging", "Staging", "192.168.1.10", 22, "ubuntu"),
        ]
    }

    pub fn address(&self) -> String {
        format!("{}@{}:{}", self.username, self.host, self.port)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
pub struct ConnectionRoute {
    #[serde(default)]
    pub upstream_proxy: Option<ProxyConfig>,
    #[serde(default)]
    pub jump_host_ids: Vec<String>,
}

impl ConnectionRoute {
    pub fn is_direct(&self) -> bool {
        self.upstream_proxy.is_none() && self.jump_host_ids.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProxyConfig {
    HttpConnect {
        host: String,
        port: u16,
        #[serde(default)]
        username: Option<String>,
    },
    Socks5 {
        host: String,
        port: u16,
        #[serde(default)]
        username: Option<String>,
    },
    ProxyCommand {
        #[serde(default)]
        command_digest: String,
        #[serde(default)]
        approved_digest: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProfileSource {
    OpenSsh {
        root_path: PathBuf,
        alias: String,
        last_import_digest: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LanguageMode {
    #[default]
    System,
    EnUs,
    ZhCn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeMode {
    #[default]
    System,

    Light,

    Dark,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TabLayout {
    Horizontal,

    #[default]
    Vertical,
}

pub const DEFAULT_MAX_PARALLEL_TRANSFERS: u8 = 4;
pub const MAX_PARALLEL_TRANSFERS: u8 = 8;
pub const MAX_TRANSFER_RATE_MIB_PER_SECOND: u32 = 10_000;
pub const DEFAULT_TERMINAL_FONT_SIZE: u16 = 14;
pub const MIN_TERMINAL_FONT_SIZE: u16 = 8;
pub const MAX_TERMINAL_FONT_SIZE: u16 = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub struct TransferSettings {
    #[serde(default)]
    pub rate_limit_mib_per_second: u32,
    #[serde(default = "default_max_parallel_transfers")]
    pub max_parallel_transfers: u8,
}

impl TransferSettings {
    pub fn normalized(self) -> Self {
        Self {
            rate_limit_mib_per_second: self
                .rate_limit_mib_per_second
                .min(MAX_TRANSFER_RATE_MIB_PER_SECOND),
            max_parallel_transfers: self.max_parallel_transfers.clamp(1, MAX_PARALLEL_TRANSFERS),
        }
    }

    pub fn bytes_per_second(self) -> Option<u64> {
        (self.rate_limit_mib_per_second > 0)
            .then_some(u64::from(self.rate_limit_mib_per_second) * 1024 * 1024)
    }
}

impl Default for TransferSettings {
    fn default() -> Self {
        Self {
            rate_limit_mib_per_second: 0,
            max_parallel_transfers: DEFAULT_MAX_PARALLEL_TRANSFERS,
        }
    }
}

const fn default_max_parallel_transfers() -> u8 {
    DEFAULT_MAX_PARALLEL_TRANSFERS
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct TerminalSettings {
    #[serde(default)]
    pub font_family: Option<String>,
    #[serde(default = "default_terminal_font_size")]
    pub font_size: u16,
}

impl TerminalSettings {
    pub fn normalized(mut self) -> Self {
        self.font_family = self
            .font_family
            .map(|family| family.trim().to_owned())
            .filter(|family| !family.is_empty());
        self.font_size = self
            .font_size
            .clamp(MIN_TERMINAL_FONT_SIZE, MAX_TERMINAL_FONT_SIZE);
        self
    }
}

impl Default for TerminalSettings {
    fn default() -> Self {
        Self {
            font_family: None,
            font_size: DEFAULT_TERMINAL_FONT_SIZE,
        }
    }
}

const fn default_terminal_font_size() -> u16 {
    DEFAULT_TERMINAL_FONT_SIZE
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuthConfig {
    #[default]
    Password,

    None,

    PrivateKey {
        path: PathBuf,
    },

    Agent,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_profile_defaults_to_password_authentication() {
        let json = r#"{
            "id": "server-1",
            "name": "Server",
            "host": "127.0.0.1",
            "port": 22,
            "username": "root"
        }"#;

        let profile: ConnectionProfile =
            serde_json::from_str(json).expect("old profile should remain valid");

        assert_eq!(profile.auth, AuthConfig::Password);
        assert!(profile.route.is_direct());
        assert_eq!(profile.source, None);
    }

    #[test]
    fn routed_profile_round_trip_never_contains_proxy_secrets() {
        let mut profile = ConnectionProfile::new("server-1", "Server", "host", 22, "user");
        profile.route = ConnectionRoute {
            upstream_proxy: Some(ProxyConfig::ProxyCommand {
                command_digest: "command-digest".into(),
                approved_digest: Some("sha256:example".into()),
            }),
            jump_host_ids: Vec::new(),
        };
        profile.source = Some(ProfileSource::OpenSsh {
            root_path: PathBuf::from("/Users/test/.ssh/config"),
            alias: "server".into(),
            last_import_digest: "import-digest".into(),
        });

        let json = serde_json::to_string(&profile).unwrap();
        let decoded: ConnectionProfile = serde_json::from_str(&json).unwrap();

        assert_eq!(decoded, profile);
        assert!(!json.contains("proxy-password"));
        assert!(!json.contains("ProxyCommand nc"));
    }

    #[test]
    fn language_mode_serializes_as_a_stable_value() {
        assert_eq!(
            serde_json::to_string(&LanguageMode::ZhCn).unwrap(),
            r#""zh_cn""#
        );
        assert_eq!(
            serde_json::from_str::<LanguageMode>(r#""en_us""#).unwrap(),
            LanguageMode::EnUs
        );
    }

    #[test]
    fn private_key_configuration_survives_json_round_trip() {
        let mut profile = ConnectionProfile::new("server-1", "Server", "127.0.0.1", 22, "root");

        profile.auth = AuthConfig::PrivateKey {
            path: PathBuf::from("/Users/test/.ssh/id_ed25519"),
        };

        let json = serde_json::to_string(&profile).expect("profile should serialize");
        let loaded: ConnectionProfile =
            serde_json::from_str(&json).expect("profile should deserialize");

        assert_eq!(loaded, profile);
    }

    #[test]
    fn theme_mode_serializes_as_a_stable_lowercase_value() {
        assert_eq!(
            serde_json::to_string(&ThemeMode::System).unwrap(),
            r#""system""#
        );
        assert_eq!(
            serde_json::from_str::<ThemeMode>(r#""light""#).unwrap(),
            ThemeMode::Light
        );
    }

    #[test]
    fn tab_layout_serializes_as_a_stable_lowercase_value() {
        assert_eq!(
            serde_json::to_string(&TabLayout::Horizontal).unwrap(),
            r#""horizontal""#
        );
        assert_eq!(
            serde_json::from_str::<TabLayout>(r#""vertical""#).unwrap(),
            TabLayout::Vertical
        );
    }

    #[test]
    fn transfer_settings_default_and_normalize_limits() {
        assert_eq!(
            TransferSettings::default().max_parallel_transfers,
            DEFAULT_MAX_PARALLEL_TRANSFERS
        );
        assert_eq!(
            TransferSettings {
                rate_limit_mib_per_second: u32::MAX,
                max_parallel_transfers: 0,
            }
            .normalized(),
            TransferSettings {
                rate_limit_mib_per_second: MAX_TRANSFER_RATE_MIB_PER_SECOND,
                max_parallel_transfers: 1,
            }
        );
        assert_eq!(
            TransferSettings {
                rate_limit_mib_per_second: 20,
                max_parallel_transfers: 4,
            }
            .bytes_per_second(),
            Some(20 * 1024 * 1024)
        );
        assert_eq!(TransferSettings::default().bytes_per_second(), None);
    }

    #[test]
    fn terminal_settings_default_and_normalize_values() {
        assert_eq!(
            TerminalSettings::default(),
            TerminalSettings {
                font_family: None,
                font_size: DEFAULT_TERMINAL_FONT_SIZE,
            }
        );
        assert_eq!(
            TerminalSettings {
                font_family: Some("  Menlo  ".into()),
                font_size: u16::MAX,
            }
            .normalized(),
            TerminalSettings {
                font_family: Some("Menlo".into()),
                font_size: MAX_TERMINAL_FONT_SIZE,
            }
        );
        assert_eq!(
            TerminalSettings {
                font_family: Some(" ".into()),
                font_size: 0,
            }
            .normalized(),
            TerminalSettings {
                font_family: None,
                font_size: MIN_TERMINAL_FONT_SIZE,
            }
        );
    }

    #[test]
    fn no_password_authentication_survives_json_round_trip() {
        let mut profile = ConnectionProfile::new("server-1", "Server", "host", 22, "user");
        profile.auth = AuthConfig::None;

        let json = serde_json::to_string(&profile).unwrap();
        let decoded: ConnectionProfile = serde_json::from_str(&json).unwrap();

        assert_eq!(decoded.auth, AuthConfig::None);
        assert!(json.contains(r#""type":"none""#));
    }
}
