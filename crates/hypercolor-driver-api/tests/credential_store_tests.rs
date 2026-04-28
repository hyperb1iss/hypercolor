use aes_gcm::aead::Aead;
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use hypercolor_driver_api::{CredentialStore, Credentials};
use tempfile::tempdir;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

const STORE_FILE_NAME: &str = "credentials.json.enc";
const SEED_FILE_NAME: &str = ".credential_seed";
const TEST_SEED: [u8; 32] = [0xA5; 32];
const TEST_NONCE: [u8; 12] = [0x5A; 12];

#[tokio::test]
async fn credential_store_round_trips_and_reopens() -> TestResult {
    let tempdir = tempdir()?;
    let store = CredentialStore::open(tempdir.path()).await?;

    store
        .store_json(
            "alpha:bridge-1",
            serde_json::json!({
                "api_key": "api-key",
                "client_key": "client-key",
            }),
        )
        .await?;
    store
        .store_json(
            "beta:panel-1",
            serde_json::json!({
                "auth_token": "panel-token",
            }),
        )
        .await?;
    store
        .store_json(
            "gamma:living-room",
            serde_json::json!({
                "username": "user",
                "password": "secret",
                "token": null,
            }),
        )
        .await?;

    assert_eq!(
        store.get_json("alpha:bridge-1").await,
        Some(serde_json::json!({
            "api_key": "api-key",
            "client_key": "client-key",
        }))
    );
    assert_eq!(
        store.keys().await,
        vec![
            "alpha:bridge-1".to_owned(),
            "beta:panel-1".to_owned(),
            "gamma:living-room".to_owned(),
        ]
    );

    let reopened = CredentialStore::open(tempdir.path()).await?;
    assert_eq!(
        reopened.get_json("beta:panel-1").await,
        Some(serde_json::json!({
            "auth_token": "panel-token",
        }))
    );
    assert_eq!(
        reopened.get_json("gamma:living-room").await,
        Some(serde_json::json!({
            "username": "user",
            "password": "secret",
            "token": null,
        }))
    );

    Ok(())
}

#[tokio::test]
async fn credential_store_remove_updates_persisted_state() -> TestResult {
    let tempdir = tempdir()?;
    let store = CredentialStore::open(tempdir.path()).await?;

    store
        .store(
            "custom:test",
            Credentials::new("custom", serde_json::json!({ "token": "value" })),
        )
        .await?;
    store.remove("custom:test").await?;

    assert_eq!(store.get("custom:test").await, None);

    let reopened = CredentialStore::open(tempdir.path()).await?;
    assert_eq!(reopened.get("custom:test").await, None);
    assert!(reopened.keys().await.is_empty());

    Ok(())
}

#[tokio::test]
async fn credential_store_exposes_driver_json_payloads() -> TestResult {
    let tempdir = tempdir()?;
    let store = CredentialStore::open(tempdir.path()).await?;

    store
        .store_json(
            "alpha:bridge-1",
            serde_json::json!({
                "api_key": "api-key",
                "client_key": "client-key",
            }),
        )
        .await?;
    store
        .store_json(
            "custom-driver:device-1",
            serde_json::json!({
                "secret": "driver-owned",
                "shape": ["opaque", "to", "host"],
            }),
        )
        .await?;

    assert_eq!(
        store.get_json("alpha:bridge-1").await,
        Some(serde_json::json!({
            "api_key": "api-key",
            "client_key": "client-key",
        }))
    );
    assert_eq!(
        store.get_json("custom-driver:device-1").await,
        Some(serde_json::json!({
            "secret": "driver-owned",
            "shape": ["opaque", "to", "host"],
        }))
    );

    Ok(())
}

#[tokio::test]
async fn credential_store_keeps_driver_json_opaque() -> TestResult {
    let tempdir = tempdir()?;
    let store = CredentialStore::open(tempdir.path()).await?;

    store
        .store_json("alpha:strip", serde_json::json!({}))
        .await?;

    assert_eq!(
        store.get_json("alpha:strip").await,
        Some(serde_json::json!({}))
    );
    assert_eq!(
        store.get("alpha:strip").await,
        Some(Credentials::new("alpha", serde_json::json!({})))
    );

    Ok(())
}

#[tokio::test]
async fn credential_store_migrates_opaque_payloads_from_existing_store() -> TestResult {
    let tempdir = tempdir()?;
    write_encrypted_store(
        tempdir.path(),
        &serde_json::json!({
            "alpha:bridge-1": {
                "api_key": "api-key",
                "client_key": "client-key"
            },
            "beta:panel-1": {
                "auth_token": "panel-token"
            }
        }),
    )?;

    let store = CredentialStore::open(tempdir.path()).await?;

    assert_eq!(
        store.get("alpha:bridge-1").await,
        Some(Credentials::new(
            "alpha",
            serde_json::json!({
                "api_key": "api-key",
                "client_key": "client-key"
            })
        ))
    );
    assert_eq!(
        store.get_json("beta:panel-1").await,
        Some(serde_json::json!({
            "auth_token": "panel-token"
        }))
    );

    let normalized = decrypt_store(tempdir.path())?;
    assert_eq!(
        normalized["alpha:bridge-1"]["backend_id"],
        serde_json::json!("alpha")
    );
    assert_eq!(
        normalized["alpha:bridge-1"]["data"],
        serde_json::json!({
            "api_key": "api-key",
            "client_key": "client-key"
        })
    );

    Ok(())
}

#[tokio::test]
async fn credential_store_keeps_plaintext_out_of_encrypted_file() -> TestResult {
    let tempdir = tempdir()?;
    let store = CredentialStore::open(tempdir.path()).await?;

    store
        .store_json(
            "alpha:bridge-1",
            serde_json::json!({
                "api_key": "visible-api-key",
                "client_key": "visible-client-key",
            }),
        )
        .await?;

    let raw = tokio::fs::read(tempdir.path().join("credentials.json.enc")).await?;
    let raw_text = String::from_utf8_lossy(&raw);
    assert!(
        !raw_text.contains("visible-api-key"),
        "encrypted store should not leak plaintext API keys"
    );
    assert!(
        !raw_text.contains("visible-client-key"),
        "encrypted store should not leak plaintext client keys"
    );

    Ok(())
}

fn write_encrypted_store(root: &std::path::Path, value: &serde_json::Value) -> TestResult {
    std::fs::write(root.join(SEED_FILE_NAME), TEST_SEED)?;
    let cipher = Aes256Gcm::new_from_slice(&TEST_SEED).expect("test key length is valid");
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&TEST_NONCE),
            serde_json::to_vec_pretty(value)?.as_ref(),
        )
        .expect("test payload should encrypt");
    let mut payload = Vec::with_capacity(TEST_NONCE.len() + ciphertext.len());
    payload.extend_from_slice(&TEST_NONCE);
    payload.extend_from_slice(&ciphertext);
    std::fs::write(root.join(STORE_FILE_NAME), payload)?;
    Ok(())
}

fn decrypt_store(root: &std::path::Path) -> TestResult<serde_json::Value> {
    let payload = std::fs::read(root.join(STORE_FILE_NAME))?;
    let cipher = Aes256Gcm::new_from_slice(&TEST_SEED).expect("test key length is valid");
    let plaintext = cipher
        .decrypt(Nonce::from_slice(&payload[..12]), &payload[12..])
        .expect("test payload should decrypt");
    Ok(serde_json::from_slice(&plaintext)?)
}
