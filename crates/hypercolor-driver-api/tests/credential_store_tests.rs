use hypercolor_driver_api::{CredentialStore, Credentials};
use tempfile::tempdir;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

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
async fn credential_store_scopes_driver_json_payloads() -> TestResult {
    let tempdir = tempdir()?;
    let store = CredentialStore::open(tempdir.path()).await?;

    store
        .store_driver_json(
            "alpha",
            "account",
            serde_json::json!({ "api_key": "alpha-key" }),
        )
        .await?;
    store
        .store_driver_json(
            "beta",
            "account",
            serde_json::json!({ "api_key": "beta-key" }),
        )
        .await?;

    assert_eq!(
        store.get_driver_json("alpha", "account").await,
        Some(serde_json::json!({ "api_key": "alpha-key" }))
    );
    assert_eq!(
        store.get_driver_json("beta", "account").await,
        Some(serde_json::json!({ "api_key": "beta-key" }))
    );

    store.remove_driver("alpha", "account").await?;
    assert_eq!(store.get_driver_json("alpha", "account").await, None);
    assert_eq!(
        store.get_driver_json("beta", "account").await,
        Some(serde_json::json!({ "api_key": "beta-key" }))
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
