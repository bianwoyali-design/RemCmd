use std::{error::Error, fmt};

use secrecy::{ExposeSecret, SecretString};

const KEYRING_SERVICE: &str = "dev.remcmd.ssh-credentials";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialKind {
    Password,
    PrivateKeyPassphrase,
}

impl CredentialKind {
    fn account_suffix(self) -> &'static str {
        match self {
            Self::Password => "password",
            Self::PrivateKeyPassphrase => "private-key-passphrase",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CredentialStoreError {
    message: String,
}

impl CredentialStoreError {
    fn new(operation: &str, error: impl fmt::Display) -> Self {
        Self {
            message: format!("Failed to {operation} a credential in the system keychain: {error}"),
        }
    }

    #[cfg(not(any(
        target_os = "macos",
        target_os = "windows",
        target_os = "linux",
        target_os = "freebsd"
    )))]
    fn unsupported() -> Self {
        Self {
            message: "The system keychain is not supported on this platform".into(),
        }
    }
}

impl fmt::Display for CredentialStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for CredentialStoreError {}

pub fn load_credential(
    profile_id: &str,
    kind: CredentialKind,
) -> Result<Option<SecretString>, CredentialStoreError> {
    load_with(&SystemCredentialBackend, profile_id, kind)
}

pub fn save_credential(
    profile_id: &str,
    kind: CredentialKind,
    secret: &SecretString,
) -> Result<(), CredentialStoreError> {
    save_with(&SystemCredentialBackend, profile_id, kind, secret)
}

pub fn delete_credential(
    profile_id: &str,
    kind: CredentialKind,
) -> Result<(), CredentialStoreError> {
    delete_with(&SystemCredentialBackend, profile_id, kind)
}

pub fn delete_profile_credentials(profile_id: &str) -> Result<(), CredentialStoreError> {
    delete_profile_with(&SystemCredentialBackend, profile_id)
}

fn credential_account(profile_id: &str, kind: CredentialKind) -> String {
    format!("profile:{profile_id}:{}", kind.account_suffix())
}

trait CredentialBackend {
    fn load(&self, account: &str) -> Result<Option<String>, CredentialStoreError>;
    fn save(&self, account: &str, secret: &str) -> Result<(), CredentialStoreError>;
    fn delete(&self, account: &str) -> Result<(), CredentialStoreError>;
}

fn load_with(
    backend: &impl CredentialBackend,
    profile_id: &str,
    kind: CredentialKind,
) -> Result<Option<SecretString>, CredentialStoreError> {
    backend
        .load(&credential_account(profile_id, kind))
        .map(|secret| secret.map(|secret| SecretString::new(secret.into_boxed_str())))
}

fn save_with(
    backend: &impl CredentialBackend,
    profile_id: &str,
    kind: CredentialKind,
    secret: &SecretString,
) -> Result<(), CredentialStoreError> {
    backend.save(
        &credential_account(profile_id, kind),
        secret.expose_secret(),
    )
}

fn delete_with(
    backend: &impl CredentialBackend,
    profile_id: &str,
    kind: CredentialKind,
) -> Result<(), CredentialStoreError> {
    backend.delete(&credential_account(profile_id, kind))
}

fn delete_profile_with(
    backend: &impl CredentialBackend,
    profile_id: &str,
) -> Result<(), CredentialStoreError> {
    let password_result = delete_with(backend, profile_id, CredentialKind::Password);
    let passphrase_result = delete_with(backend, profile_id, CredentialKind::PrivateKeyPassphrase);

    password_result.and(passphrase_result)
}

struct SystemCredentialBackend;

#[cfg(any(
    target_os = "macos",
    target_os = "windows",
    target_os = "linux",
    target_os = "freebsd"
))]
impl SystemCredentialBackend {
    fn entry(account: &str) -> Result<keyring::Entry, CredentialStoreError> {
        keyring::Entry::new(KEYRING_SERVICE, account)
            .map_err(|error| CredentialStoreError::new("open", error))
    }
}

#[cfg(any(
    target_os = "macos",
    target_os = "windows",
    target_os = "linux",
    target_os = "freebsd"
))]
impl CredentialBackend for SystemCredentialBackend {
    fn load(&self, account: &str) -> Result<Option<String>, CredentialStoreError> {
        let entry = Self::entry(account)?;
        match entry.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(CredentialStoreError::new("read", error)),
        }
    }

    fn save(&self, account: &str, secret: &str) -> Result<(), CredentialStoreError> {
        Self::entry(account)?
            .set_password(secret)
            .map_err(|error| CredentialStoreError::new("save", error))
    }

    fn delete(&self, account: &str) -> Result<(), CredentialStoreError> {
        match Self::entry(account)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(CredentialStoreError::new("delete", error)),
        }
    }
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "windows",
    target_os = "linux",
    target_os = "freebsd"
)))]
impl CredentialBackend for SystemCredentialBackend {
    fn load(&self, _account: &str) -> Result<Option<String>, CredentialStoreError> {
        Err(CredentialStoreError::unsupported())
    }

    fn save(&self, _account: &str, _secret: &str) -> Result<(), CredentialStoreError> {
        Err(CredentialStoreError::unsupported())
    }

    fn delete(&self, _account: &str) -> Result<(), CredentialStoreError> {
        Err(CredentialStoreError::unsupported())
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, sync::Mutex};

    use super::*;

    #[derive(Default)]
    struct MemoryBackend {
        entries: Mutex<HashMap<String, String>>,
    }

    impl CredentialBackend for MemoryBackend {
        fn load(&self, account: &str) -> Result<Option<String>, CredentialStoreError> {
            Ok(self.entries.lock().unwrap().get(account).cloned())
        }

        fn save(&self, account: &str, secret: &str) -> Result<(), CredentialStoreError> {
            self.entries
                .lock()
                .unwrap()
                .insert(account.into(), secret.into());
            Ok(())
        }

        fn delete(&self, account: &str) -> Result<(), CredentialStoreError> {
            self.entries.lock().unwrap().remove(account);
            Ok(())
        }
    }

    #[test]
    fn credentials_round_trip_without_exposing_them_in_profile_data() {
        let backend = MemoryBackend::default();
        let secret = SecretString::new("test-only-secret".into());

        save_with(&backend, "server-1", CredentialKind::Password, &secret).unwrap();

        let loaded = load_with(&backend, "server-1", CredentialKind::Password)
            .unwrap()
            .unwrap();
        assert_eq!(loaded.expose_secret(), secret.expose_secret());
    }

    #[test]
    fn missing_credentials_are_not_errors() {
        let backend = MemoryBackend::default();

        assert!(
            load_with(&backend, "missing", CredentialKind::Password)
                .unwrap()
                .is_none()
        );
        delete_with(&backend, "missing", CredentialKind::Password).unwrap();
    }

    #[test]
    fn deleting_a_profile_removes_both_credential_kinds() {
        let backend = MemoryBackend::default();
        let secret = SecretString::new("test-only-secret".into());
        save_with(&backend, "server-1", CredentialKind::Password, &secret).unwrap();
        save_with(
            &backend,
            "server-1",
            CredentialKind::PrivateKeyPassphrase,
            &secret,
        )
        .unwrap();

        delete_profile_with(&backend, "server-1").unwrap();

        assert!(
            load_with(&backend, "server-1", CredentialKind::Password)
                .unwrap()
                .is_none()
        );
        assert!(
            load_with(&backend, "server-1", CredentialKind::PrivateKeyPassphrase,)
                .unwrap()
                .is_none()
        );
    }
}
