use std::path::PathBuf;

use secrecy::SecretString;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthMethodKind {
    None,
    Password,
    PrivateKey,
    Agent,
}

/// Authentication information used for one SSH connection.
///
/// This type intentionally does not implement Serialize, Deserialize,
/// Debug, or Clone to reduce accidental credential exposure.
pub enum AuthMethod {
    /// Request SSH access without a password, key, or agent.
    None,

    /// Authenticate with the account password.
    Password { password: SecretString },

    /// Authenticate with a private-key file.
    PrivateKey {
        path: PathBuf,
        passphrase: Option<SecretString>,
    },

    /// Authenticate through the local SSH Agent.
    Agent,
}

impl AuthMethod {
    pub const fn kind(&self) -> AuthMethodKind {
        match self {
            Self::None => AuthMethodKind::None,
            Self::Password { .. } => AuthMethodKind::Password,
            Self::PrivateKey { .. } => AuthMethodKind::PrivateKey,
            Self::Agent => AuthMethodKind::Agent,
        }
    }

    pub fn password(password: String) -> Self {
        Self::Password {
            // Consume String and convert it into the Box<str>
            // required by SecretString.
            password: SecretString::new(password.into_boxed_str()),
        }
    }

    pub fn private_key(path: PathBuf, passphrase: Option<String>) -> Self {
        Self::PrivateKey {
            path,

            // Convert the passphrase only when one was provided.
            passphrase: passphrase.map(|passphrase| SecretString::new(passphrase.into_boxed_str())),
        }
    }
}

#[cfg(test)]
mod tests {
    use secrecy::ExposeSecret;

    use super::*;

    #[test]
    fn password_constructor_wraps_password_as_secret() {
        let auth = AuthMethod::password("test-password".to_owned());

        let AuthMethod::Password { password } = auth else {
            panic!("expected password authentication")
        };

        assert_eq!(password.expose_secret(), "test-password");
    }

    #[test]
    fn private_key_constructor_preserves_configuration() {
        let expected_path = PathBuf::from("/Users/test/.ssh/id_ed25519");

        let auth =
            AuthMethod::private_key(expected_path.clone(), Some("test-passphrase".to_owned()));

        let AuthMethod::PrivateKey { path, passphrase } = auth else {
            panic!("expected private-key authentication");
        };

        assert_eq!(path, expected_path);
        assert_eq!(
            passphrase.as_ref().map(|secret| secret.expose_secret()),
            Some("test-passphrase")
        );
    }
}
