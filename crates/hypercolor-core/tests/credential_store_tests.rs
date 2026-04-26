use hypercolor_core::device::net::{CredentialStore, Credentials};
use tempfile::tempdir;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[tokio::test]
async fn credential_store_round_trips_and_reopens() -> TestResult {
    let tempdir = tempdir()?;
    let store = CredentialStore::open(tempdir.path()).await?;

    store
        .store_json(
            "hue:bridge-1",
            serde_json::json!({
                "api_key": "api-key",
                "client_key": "client-key",
            }),
        )
        .await?;
    store
        .store_json(
            "nanoleaf:panel-1",
            serde_json::json!({
                "auth_token": "nano-token",
            }),
        )
        .await?;
    store
        .store_json(
            "wled:living-room",
            serde_json::json!({
                "username": "wled",
                "password": "secret",
                "token": null,
            }),
        )
        .await?;

    assert_eq!(
        store.get_json("hue:bridge-1").await,
        Some(serde_json::json!({
            "api_key": "api-key",
            "client_key": "client-key",
        }))
    );
    assert_eq!(
        store.keys().await,
        vec![
            "hue:bridge-1".to_owned(),
            "nanoleaf:panel-1".to_owned(),
            "wled:living-room".to_owned(),
        ]
    );

    let reopened = CredentialStore::open(tempdir.path()).await?;
    assert_eq!(
        reopened.get_json("nanoleaf:panel-1").await,
        Some(serde_json::json!({
            "auth_token": "nano-token",
        }))
    );
    assert_eq!(
        reopened.get_json("wled:living-room").await,
        Some(serde_json::json!({
            "username": "wled",
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
            "hue:bridge-1",
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
        store.get_json("hue:bridge-1").await,
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
        .store_json("wled:strip", serde_json::json!({}))
        .await?;

    assert_eq!(
        store.get_json("wled:strip").await,
        Some(serde_json::json!({}))
    );
    assert_eq!(
        store.get("wled:strip").await,
        Some(Credentials::new("wled", serde_json::json!({})))
    );

    Ok(())
}

#[test]
fn credentials_migrate_legacy_typed_payloads_to_opaque_driver_json() -> TestResult {
    let credentials: Credentials = serde_json::from_value(serde_json::json!({
        "kind": "hue_bridge",
        "api_key": "api-key",
        "client_key": "client-key",
    }))?;

    assert_eq!(credentials.backend_id, "hue");
    assert_eq!(
        credentials.into_driver_json(),
        serde_json::json!({
            "api_key": "api-key",
            "client_key": "client-key",
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
            "hue:bridge-1",
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
