use std::collections::HashMap;
use std::sync::Mutex;

use hypercolor_cloud_client::{
    CloudClientError, CloudSecretKey, RefreshTokenOwner, SecretStore, delete_daemon_identity,
    delete_refresh_token, load_or_create_identity, load_refresh_token, persist_identity,
    store_refresh_token,
};
use hypercolor_daemon_link::IdentityPrivateKey;

#[derive(Debug, Default)]
struct MemorySecretStore {
    values: Mutex<HashMap<CloudSecretKey, String>>,
}

impl SecretStore for MemorySecretStore {
    fn get_secret(&self, key: CloudSecretKey) -> Result<Option<String>, CloudClientError> {
        Ok(self
            .values
            .lock()
            .expect("memory secret store lock should not be poisoned")
            .get(&key)
            .cloned())
    }

    fn put_secret(&self, key: CloudSecretKey, value: &str) -> Result<(), CloudClientError> {
        self.values
            .lock()
            .expect("memory secret store lock should not be poisoned")
            .insert(key, value.to_owned());
        Ok(())
    }

    fn delete_secret(&self, key: CloudSecretKey) -> Result<(), CloudClientError> {
        self.values
            .lock()
            .expect("memory secret store lock should not be poisoned")
            .remove(&key);
        Ok(())
    }
}

#[test]
fn load_or_create_identity_generates_and_persists_daemon_identity() {
    let store = MemorySecretStore::default();
    let identity = load_or_create_identity(&store).expect("identity should generate");
    let reloaded = load_or_create_identity(&store).expect("identity should reload");

    assert_eq!(identity.daemon_id(), reloaded.daemon_id());
    assert_eq!(
        identity.keypair().public_key(),
        reloaded.keypair().public_key()
    );
    assert!(
        IdentityPrivateKey::new(
            store
                .get_secret(CloudSecretKey::DaemonIdentityKey)
                .expect("key read should succeed")
                .expect("identity key should exist")
        )
        .is_ok()
    );
}

#[test]
fn load_or_create_identity_rejects_partial_persisted_identity() {
    let store = MemorySecretStore::default();
    store
        .put_secret(
            CloudSecretKey::DaemonId,
            "018f4c36-4a44-7cc9-9f57-0d2e9224d2f1",
        )
        .expect("seed daemon id");

    assert!(matches!(
        load_or_create_identity(&store),
        Err(CloudClientError::IncompleteIdentity)
    ));
}

#[test]
fn persist_identity_rolls_back_daemon_id_when_key_write_fails() {
    #[derive(Debug, Default)]
    struct FailingKeyStore(MemorySecretStore);

    impl SecretStore for FailingKeyStore {
        fn get_secret(&self, key: CloudSecretKey) -> Result<Option<String>, CloudClientError> {
            self.0.get_secret(key)
        }

        fn put_secret(&self, key: CloudSecretKey, value: &str) -> Result<(), CloudClientError> {
            if key == CloudSecretKey::DaemonIdentityKey {
                return Err(CloudClientError::InvalidDaemonId("forced failure".into()));
            }
            self.0.put_secret(key, value)
        }

        fn delete_secret(&self, key: CloudSecretKey) -> Result<(), CloudClientError> {
            self.0.delete_secret(key)
        }
    }

    let identity =
        load_or_create_identity(&MemorySecretStore::default()).expect("identity should generate");
    let store = FailingKeyStore::default();

    assert!(persist_identity(&store, &identity).is_err());
    assert!(
        store
            .get_secret(CloudSecretKey::DaemonId)
            .expect("daemon id read should succeed")
            .is_none()
    );
}

#[test]
fn refresh_tokens_are_scoped_by_owner() {
    let store = MemorySecretStore::default();

    store_refresh_token(&store, RefreshTokenOwner::Daemon, "daemon-token")
        .expect("daemon token should store");
    store_refresh_token(&store, RefreshTokenOwner::Cli, "cli-token")
        .expect("cli token should store");

    assert_eq!(
        load_refresh_token(&store, RefreshTokenOwner::Daemon).expect("daemon token should load"),
        Some("daemon-token".into())
    );
    assert_eq!(
        load_refresh_token(&store, RefreshTokenOwner::Cli).expect("cli token should load"),
        Some("cli-token".into())
    );

    delete_refresh_token(&store, RefreshTokenOwner::Daemon).expect("daemon token should delete");
    assert_eq!(
        load_refresh_token(&store, RefreshTokenOwner::Daemon).expect("daemon token should load"),
        None
    );
    assert_eq!(
        load_refresh_token(&store, RefreshTokenOwner::Cli).expect("cli token should load"),
        Some("cli-token".into())
    );
}

#[test]
fn delete_daemon_identity_removes_both_identity_parts() {
    let store = MemorySecretStore::default();
    load_or_create_identity(&store).expect("identity should generate");

    delete_daemon_identity(&store).expect("identity should delete");

    assert!(
        store
            .get_secret(CloudSecretKey::DaemonId)
            .expect("daemon id read should succeed")
            .is_none()
    );
    assert!(
        store
            .get_secret(CloudSecretKey::DaemonIdentityKey)
            .expect("identity key read should succeed")
            .is_none()
    );
}
