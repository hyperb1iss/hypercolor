use hypercolor_daemon_link::{IdentityKeypair, IdentityPrivateKey};
use keyring_core::{Entry, Error as KeyringError};
use uuid::Uuid;

use crate::CloudClientError;

pub const KEYRING_SERVICE: &str = "hypercolor.cloud";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CloudSecretKey {
    DaemonId,
    DaemonIdentityKey,
    DaemonRefreshToken,
    CliRefreshToken,
}

impl CloudSecretKey {
    #[must_use]
    pub const fn account(self) -> &'static str {
        match self {
            Self::DaemonId => "daemon.id",
            Self::DaemonIdentityKey => "daemon.identity_key",
            Self::DaemonRefreshToken => "daemon.refresh_token",
            Self::CliRefreshToken => "cli.refresh_token",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshTokenOwner {
    Daemon,
    Cli,
}

impl RefreshTokenOwner {
    const fn secret_key(self) -> CloudSecretKey {
        match self {
            Self::Daemon => CloudSecretKey::DaemonRefreshToken,
            Self::Cli => CloudSecretKey::CliRefreshToken,
        }
    }
}

pub trait SecretStore {
    fn get_secret(&self, key: CloudSecretKey) -> Result<Option<String>, CloudClientError>;
    fn put_secret(&self, key: CloudSecretKey, value: &str) -> Result<(), CloudClientError>;
    fn delete_secret(&self, key: CloudSecretKey) -> Result<(), CloudClientError>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct KeyringSecretStore;

impl KeyringSecretStore {
    pub fn new_native() -> Result<Self, CloudClientError> {
        set_native_store()?;
        Ok(Self)
    }

    fn entry(key: CloudSecretKey) -> Result<Entry, CloudClientError> {
        Ok(Entry::new(KEYRING_SERVICE, key.account())?)
    }
}

#[cfg(target_os = "linux")]
fn set_native_store() -> Result<(), CloudClientError> {
    keyring_core::set_default_store(dbus_secret_service_keyring_store::Store::new()?);
    Ok(())
}

#[cfg(target_os = "macos")]
fn set_native_store() -> Result<(), CloudClientError> {
    keyring_core::set_default_store(apple_native_keyring_store::keychain::Store::new()?);
    Ok(())
}

#[cfg(target_os = "windows")]
fn set_native_store() -> Result<(), CloudClientError> {
    keyring_core::set_default_store(windows_native_keyring_store::Store::new()?);
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn set_native_store() -> Result<(), CloudClientError> {
    Err(CloudClientError::CredentialStore(
        KeyringError::NotSupportedByStore("native credential store is unavailable".to_owned()),
    ))
}

impl SecretStore for KeyringSecretStore {
    fn get_secret(&self, key: CloudSecretKey) -> Result<Option<String>, CloudClientError> {
        match Self::entry(key)?.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(KeyringError::NoEntry) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    fn put_secret(&self, key: CloudSecretKey, value: &str) -> Result<(), CloudClientError> {
        Ok(Self::entry(key)?.set_password(value)?)
    }

    fn delete_secret(&self, key: CloudSecretKey) -> Result<(), CloudClientError> {
        match Self::entry(key)?.delete_credential() {
            Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CloudIdentity {
    daemon_id: Uuid,
    keypair: IdentityKeypair,
}

impl CloudIdentity {
    #[must_use]
    pub const fn new(daemon_id: Uuid, keypair: IdentityKeypair) -> Self {
        Self { daemon_id, keypair }
    }

    #[must_use]
    pub const fn daemon_id(&self) -> Uuid {
        self.daemon_id
    }

    #[must_use]
    pub const fn keypair(&self) -> &IdentityKeypair {
        &self.keypair
    }
}

pub fn load_or_create_identity(
    store: &impl SecretStore,
) -> Result<CloudIdentity, CloudClientError> {
    match (
        store.get_secret(CloudSecretKey::DaemonId)?,
        store.get_secret(CloudSecretKey::DaemonIdentityKey)?,
    ) {
        (Some(daemon_id), Some(private_key)) => {
            let daemon_id = Uuid::parse_str(&daemon_id)
                .map_err(|error| CloudClientError::InvalidDaemonId(error.to_string()))?;
            let private_key = IdentityPrivateKey::new(private_key)?;
            let keypair = IdentityKeypair::from_private_key(&private_key)?;
            Ok(CloudIdentity::new(daemon_id, keypair))
        }
        (None, None) => {
            let identity = CloudIdentity::new(Uuid::new_v4(), IdentityKeypair::generate());
            persist_identity(store, &identity)?;
            Ok(identity)
        }
        _ => Err(CloudClientError::IncompleteIdentity),
    }
}

pub fn persist_identity(
    store: &impl SecretStore,
    identity: &CloudIdentity,
) -> Result<(), CloudClientError> {
    store.put_secret(
        CloudSecretKey::DaemonId,
        &identity.daemon_id().hyphenated().to_string(),
    )?;
    if let Err(error) = store.put_secret(
        CloudSecretKey::DaemonIdentityKey,
        identity.keypair().private_key().as_str(),
    ) {
        let _ = store.delete_secret(CloudSecretKey::DaemonId);
        return Err(error);
    }
    Ok(())
}

pub fn store_refresh_token(
    store: &impl SecretStore,
    owner: RefreshTokenOwner,
    token: &str,
) -> Result<(), CloudClientError> {
    store.put_secret(owner.secret_key(), token)
}

pub fn load_refresh_token(
    store: &impl SecretStore,
    owner: RefreshTokenOwner,
) -> Result<Option<String>, CloudClientError> {
    store.get_secret(owner.secret_key())
}

pub fn delete_refresh_token(
    store: &impl SecretStore,
    owner: RefreshTokenOwner,
) -> Result<(), CloudClientError> {
    store.delete_secret(owner.secret_key())
}

pub fn delete_daemon_identity(store: &impl SecretStore) -> Result<(), CloudClientError> {
    store.delete_secret(CloudSecretKey::DaemonIdentityKey)?;
    store.delete_secret(CloudSecretKey::DaemonId)
}
