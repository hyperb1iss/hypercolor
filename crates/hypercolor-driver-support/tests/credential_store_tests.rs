use hypercolor_driver_support::CredentialStore;
use tempfile::tempdir;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[cfg(unix)]
fn file_mode(path: impl AsRef<std::path::Path>) -> TestResult<u32> {
    use std::os::unix::fs::PermissionsExt as _;

    Ok(std::fs::metadata(path)?.permissions().mode() & 0o777)
}

#[cfg(unix)]
fn set_file_mode(path: impl AsRef<std::path::Path>, mode: u32) -> TestResult {
    use std::os::unix::fs::PermissionsExt as _;

    let mut permissions = std::fs::metadata(path.as_ref())?.permissions();
    permissions.set_mode(mode);
    std::fs::set_permissions(path, permissions)?;
    Ok(())
}

#[tokio::test]
async fn credential_store_round_trips_and_reopens() -> TestResult {
    let tempdir = tempdir()?;
    let store = CredentialStore::open(tempdir.path()).await?;

    store
        .store_driver_json(
            "alpha",
            "bridge-1",
            serde_json::json!({
                "api_key": "api-key",
                "client_key": "client-key",
            }),
        )
        .await?;
    store
        .store_driver_json(
            "beta",
            "panel-1",
            serde_json::json!({
                "auth_token": "panel-token",
            }),
        )
        .await?;
    store
        .store_driver_json(
            "gamma",
            "living-room",
            serde_json::json!({
                "username": "user",
                "password": "secret",
                "token": null,
            }),
        )
        .await?;

    assert_eq!(
        store.get_driver_json("alpha", "bridge-1").await,
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
        reopened.get_driver_json("beta", "panel-1").await,
        Some(serde_json::json!({
            "auth_token": "panel-token",
        }))
    );
    assert_eq!(
        reopened.get_driver_json("gamma", "living-room").await,
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
        .store_driver_json("custom", "test", serde_json::json!({ "token": "value" }))
        .await?;
    store.remove_driver("custom", "test").await?;

    assert_eq!(store.get_driver_json("custom", "test").await, None);

    let reopened = CredentialStore::open(tempdir.path()).await?;
    assert_eq!(reopened.get_driver_json("custom", "test").await, None);
    assert!(reopened.keys().await.is_empty());

    Ok(())
}

#[tokio::test]
async fn credential_store_exposes_driver_json_payloads() -> TestResult {
    let tempdir = tempdir()?;
    let store = CredentialStore::open(tempdir.path()).await?;

    store
        .store_driver_json(
            "alpha",
            "bridge-1",
            serde_json::json!({
                "api_key": "api-key",
                "client_key": "client-key",
            }),
        )
        .await?;
    store
        .store_driver_json(
            "custom-driver",
            "device-1",
            serde_json::json!({
                "secret": "driver-owned",
                "shape": ["opaque", "to", "host"],
            }),
        )
        .await?;

    assert_eq!(
        store.get_driver_json("alpha", "bridge-1").await,
        Some(serde_json::json!({
            "api_key": "api-key",
            "client_key": "client-key",
        }))
    );
    assert_eq!(
        store.get_driver_json("custom-driver", "device-1").await,
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
        store.keys().await,
        vec!["alpha:account".to_owned(), "beta:account".to_owned()]
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

    let payload = serde_json::json!({
        "driver_id": "driver-owned-field",
        "data": {
            "nested": true,
        },
    });

    store
        .store_driver_json("alpha", "strip", payload.clone())
        .await?;

    assert_eq!(store.get_driver_json("alpha", "strip").await, Some(payload));
    assert_eq!(store.keys().await, vec!["alpha:strip".to_owned()]);

    Ok(())
}

#[tokio::test]
async fn credential_store_keeps_plaintext_out_of_encrypted_file() -> TestResult {
    let tempdir = tempdir()?;
    let store = CredentialStore::open(tempdir.path()).await?;

    store
        .store_driver_json(
            "alpha",
            "bridge-1",
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

#[cfg(unix)]
#[tokio::test]
async fn credential_store_creates_secret_files_with_private_permissions() -> TestResult {
    let tempdir = tempdir()?;
    let store = CredentialStore::open(tempdir.path()).await?;
    let seed_path = tempdir.path().join(".credential_seed");
    let store_path = tempdir.path().join("credentials.json.enc");

    assert_eq!(file_mode(&seed_path)?, 0o600);

    store
        .store_driver_json("alpha", "bridge-1", serde_json::json!({ "api_key": "key" }))
        .await?;

    assert_eq!(file_mode(&seed_path)?, 0o600);
    assert_eq!(file_mode(&store_path)?, 0o600);

    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn credential_store_restricts_existing_secret_file_permissions() -> TestResult {
    let tempdir = tempdir()?;
    let store = CredentialStore::open(tempdir.path()).await?;
    let seed_path = tempdir.path().join(".credential_seed");
    let store_path = tempdir.path().join("credentials.json.enc");
    store
        .store_driver_json("alpha", "bridge-1", serde_json::json!({ "api_key": "key" }))
        .await?;
    set_file_mode(&seed_path, 0o644)?;
    set_file_mode(&store_path, 0o644)?;

    let reopened = CredentialStore::open(tempdir.path()).await?;

    assert_eq!(
        reopened.get_driver_json("alpha", "bridge-1").await,
        Some(serde_json::json!({ "api_key": "key" }))
    );
    assert_eq!(file_mode(&seed_path)?, 0o600);
    assert_eq!(file_mode(&store_path)?, 0o600);

    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn credential_store_refuses_symlinked_store_and_seed() -> TestResult {
    use std::os::unix::fs::symlink;

    let tempdir = tempdir()?;
    let store = CredentialStore::open(tempdir.path()).await?;
    store
        .store_driver_json("alpha", "bridge-1", serde_json::json!({ "api_key": "key" }))
        .await?;
    drop(store);

    let store_path = tempdir.path().join("credentials.json.enc");
    let relocated = tempdir.path().join("relocated.enc");
    std::fs::rename(&store_path, &relocated)?;
    symlink(&relocated, &store_path)?;

    assert!(
        CredentialStore::open(tempdir.path()).await.is_err(),
        "a symlinked store must be refused"
    );
    assert!(
        CredentialStore::open_blocking(tempdir.path()).is_err(),
        "a symlinked store must be refused on the blocking path"
    );
    assert!(std::fs::symlink_metadata(&store_path)?.is_symlink());
    assert!(relocated.is_file(), "refusal must not disturb the target");

    std::fs::remove_file(&store_path)?;
    std::fs::rename(&relocated, &store_path)?;
    let seed_path = tempdir.path().join(".credential_seed");
    let relocated_seed = tempdir.path().join("relocated_seed");
    std::fs::rename(&seed_path, &relocated_seed)?;
    symlink(&relocated_seed, &seed_path)?;

    assert!(
        CredentialStore::open(tempdir.path()).await.is_err(),
        "a symlinked seed must be refused"
    );
    assert!(std::fs::symlink_metadata(&seed_path)?.is_symlink());

    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn failed_persist_leaves_prior_store_readable() -> TestResult {
    use std::os::unix::fs::MetadataExt as _;

    if std::fs::metadata("/")?.uid() == effective_uid() {
        eprintln!("skipping: root bypasses directory permission checks");
        return Ok(());
    }

    let tempdir = tempdir()?;
    let store = CredentialStore::open(tempdir.path()).await?;
    store
        .store_driver_json(
            "alpha",
            "bridge-1",
            serde_json::json!({ "api_key": "first" }),
        )
        .await?;

    set_file_mode(tempdir.path(), 0o500)?;
    let failed = store
        .store_driver_json(
            "alpha",
            "bridge-1",
            serde_json::json!({ "api_key": "second" }),
        )
        .await;
    set_file_mode(tempdir.path(), 0o700)?;
    assert!(failed.is_err(), "a sealed directory must fail the persist");

    let leftovers: Vec<_> = std::fs::read_dir(tempdir.path())?
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains(".tmp-"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "no temp files may remain: {leftovers:?}"
    );

    let reopened = CredentialStore::open(tempdir.path()).await?;
    assert_eq!(
        reopened.get_driver_json("alpha", "bridge-1").await,
        Some(serde_json::json!({ "api_key": "first" }))
    );

    Ok(())
}

#[cfg(unix)]
fn effective_uid() -> u32 {
    use std::os::unix::fs::MetadataExt as _;

    // The effective uid owns any file this process creates.
    let probe = tempfile::NamedTempFile::new().expect("probe file");
    std::fs::metadata(probe.path())
        .expect("probe metadata")
        .uid()
}
