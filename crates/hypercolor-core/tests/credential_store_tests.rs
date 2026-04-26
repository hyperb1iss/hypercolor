use hypercolor_core::device::net::{CredentialStore, Credentials};
use tempfile::tempdir;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[tokio::test]
async fn credential_store_round_trips_and_reopens() -> TestResult {
    let tempdir = tempdir()?;
    let store = CredentialStore::open(tempdir.path()).await?;

    store
        .store(
            "hue:bridge-1",
            Credentials::HueBridge {
                api_key: "api-key".to_owned(),
                client_key: "client-key".to_owned(),
            },
        )
        .await?;
    store
        .store(
            "nanoleaf:panel-1",
            Credentials::Nanoleaf {
                auth_token: "nano-token".to_owned(),
            },
        )
        .await?;
    store
        .store(
            "wled:living-room",
            Credentials::Wled {
                username: Some("wled".to_owned()),
                password: Some("secret".to_owned()),
                token: None,
            },
        )
        .await?;

    assert_eq!(
        store.get("hue:bridge-1").await,
        Some(Credentials::HueBridge {
            api_key: "api-key".to_owned(),
            client_key: "client-key".to_owned(),
        })
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
        reopened.get("nanoleaf:panel-1").await,
        Some(Credentials::Nanoleaf {
            auth_token: "nano-token".to_owned(),
        })
    );
    assert_eq!(
        reopened.get("wled:living-room").await,
        Some(Credentials::Wled {
            username: Some("wled".to_owned()),
            password: Some("secret".to_owned()),
            token: None,
        })
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
            Credentials::Custom {
                backend_id: "custom".to_owned(),
                data: serde_json::json!({ "token": "value" }),
            },
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
        Some(Credentials::Custom {
            backend_id: "wled".to_owned(),
            data: serde_json::json!({}),
        })
    );

    Ok(())
}

#[tokio::test]
async fn credential_store_keeps_plaintext_out_of_encrypted_file() -> TestResult {
    let tempdir = tempdir()?;
    let store = CredentialStore::open(tempdir.path()).await?;

    store
        .store(
            "hue:bridge-1",
            Credentials::HueBridge {
                api_key: "visible-api-key".to_owned(),
                client_key: "visible-client-key".to_owned(),
            },
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
