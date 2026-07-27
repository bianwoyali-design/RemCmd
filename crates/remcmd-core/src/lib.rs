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
    fn no_password_authentication_survives_json_round_trip() {
        let mut profile = ConnectionProfile::new("server-1", "Server", "host", 22, "user");
        profile.auth = AuthConfig::None;

        let json = serde_json::to_string(&profile).unwrap();
        let decoded: ConnectionProfile = serde_json::from_str(&json).unwrap();

        assert_eq!(decoded.auth, AuthConfig::None);
        assert!(json.contains(r#""type":"none""#));
    }
}
