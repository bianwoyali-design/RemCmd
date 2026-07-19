use russh::keys::{HashAlg, PublicKey};

/// Public information needed to verify an SSH server identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostKeyInfo {
    host: String,
    port: u16,
    algorithm: String,
    fingerprint: String,
}

impl HostKeyInfo {
    pub(crate) fn from_public_key(host: String, port: u16, public_key: &PublicKey) -> Self {
        Self {
            host,
            port,
            algorithm: public_key.algorithm().as_str().into(),
            fingerprint: public_key.fingerprint(HashAlg::Sha256).to_string(),
        }
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub const fn port(&self) -> u16 {
        self.port
    }

    pub fn algorithm(&self) -> &str {
        &self.algorithm
    }

    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    pub fn address(&self) -> String {
        if self.host.contains(':') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HostKeyDecision {
    Trust,
    Reject,
}

#[cfg(test)]
mod tests {
    use super::*;

    const PUBLIC_KEY: &str = "AAAAC3NzaC1lZDI1NTE5AAAAIJdD7y3aLq454yWBdwLWbieU1ebz9/cu7/QEXn9OIeZJ";

    #[test]
    fn host_key_info_uses_sha256_fingerprint() {
        let public_key =
            russh::keys::parse_public_key_base64(PUBLIC_KEY).expect("public key should parse");

        let info = HostKeyInfo::from_public_key("example.com".into(), 2222, &public_key);

        assert_eq!(info.address(), "example.com:2222");
        assert_eq!(info.algorithm(), "ssh-ed25519");
        assert!(info.fingerprint().starts_with("SHA256:"));
    }

    #[test]
    fn ipv6_address_is_unambiguous() {
        let public_key =
            russh::keys::parse_public_key_base64(PUBLIC_KEY).expect("public key should parse");

        let info = HostKeyInfo::from_public_key("2001:db8::1".into(), 22, &public_key);

        assert_eq!(info.address(), "[2001:db8::1]:22");
    }
}
