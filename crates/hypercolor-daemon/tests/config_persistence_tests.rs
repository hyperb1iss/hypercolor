//! Config writes whose file write fails stay live and say so.
//!
//! Each test makes the config file's directory read-only (0o500), so every
//! write attempt fails before it can replace the file, while the daemon's
//! other stores live in a separate writable data directory. A test skips
//! itself when the directory stays writable anyway (root, or a filesystem
//! that ignores mode bits), because the failure it exercises cannot happen
//! there.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use http::{Request, StatusCode};
use hypercolor_core::config::ConfigManager;
use hypercolor_daemon::api::local::TrustedLocalApi;
use hypercolor_daemon::app_state::{AppState, AppStateBuilder};
use hypercolor_daemon::persistence::AtomicFileWriter;

const CONFIG_FILE_NAME: &str = "hypercolor.toml";

/// A directory the test process cannot create files in until it unlocks.
struct ReadOnlyDirectory {
    path: PathBuf,
}

impl ReadOnlyDirectory {
    /// Drop write permission on `path`, or return `None` when the process
    /// can still create files there.
    fn lock(path: &Path) -> Option<Self> {
        set_mode(path, 0o500);
        let probe = path.join(".write-probe");
        if fs::File::create(&probe).is_ok() {
            drop(fs::remove_file(&probe));
            set_mode(path, 0o700);
            return None;
        }
        Some(Self {
            path: path.to_path_buf(),
        })
    }

    fn unlock(&self) {
        set_mode(&self.path, 0o700);
    }
}

impl Drop for ReadOnlyDirectory {
    fn drop(&mut self) {
        self.unlock();
    }
}

fn set_mode(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .expect("test directory permissions should change");
}

/// A daemon whose config file sits in a read-only directory.
///
/// Field order is drop order: the directory unlocks before the temporary
/// root is removed, so cleanup works even when an assertion panics.
struct Fixture {
    state: Arc<AppState>,
    manager: Arc<ConfigManager>,
    config_path: PathBuf,
    read_only: ReadOnlyDirectory,
    _root: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Option<Self> {
        let root = tempfile::tempdir().expect("temporary root should be created");
        let config_dir = root.path().join("config");
        let data_dir = root.path().join("data");
        fs::create_dir_all(&config_dir).expect("config directory should be created");
        fs::create_dir_all(&data_dir).expect("data directory should be created");
        let config_path = config_dir.join(CONFIG_FILE_NAME);
        let manager =
            Arc::new(ConfigManager::new(config_path.clone()).expect("config manager should build"));
        let state = Arc::new(
            AppStateBuilder::new(data_dir)
                .with_config_manager(Arc::clone(&manager))
                .build(),
        );
        let read_only = ReadOnlyDirectory::lock(&config_dir)?;
        Some(Self {
            state,
            manager,
            config_path,
            read_only,
            _root: root,
        })
    }

    async fn send(&self, request: Request<Body>) -> serde_json::Value {
        let response = TrustedLocalApi::new(Arc::clone(&self.state))
            .execute(request)
            .await
            .expect("trusted request should execute");
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body should read");
        serde_json::from_slice(&bytes).expect("response body should be JSON")
    }

    async fn put(&self, key: &str, value: &serde_json::Value) -> serde_json::Value {
        self.send(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/v1/config/keys/{key}?live=false"))
                .header("content-type", "application/json")
                .body(Body::from(value.to_string()))
                .expect("request should build"),
        )
        .await
    }

    async fn delete(&self, key: &str) -> serde_json::Value {
        self.send(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/config/keys/{key}?live=false"))
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
    }

    /// The `config_persistence` finding from a config-only diagnose run.
    async fn persistence_check(&self) -> serde_json::Value {
        let report = self
            .send(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/diagnose")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"checks":["config"]}"#))
                    .expect("request should build"),
            )
            .await;
        report["data"]["checks"]
            .as_array()
            .expect("diagnose checks should be an array")
            .iter()
            .find(|check| check["name"] == "config_persistence")
            .cloned()
            .expect("the config check set should report config persistence")
    }

    /// Restore write access and wait for the retry to land the file.
    fn unlock_and_converge(&self) {
        self.read_only.unlock();
        AtomicFileWriter::new(&self.config_path)
            .expect("config destination should resolve")
            .flush(Duration::from_secs(5))
            .expect("the config retry should land once the directory is writable");
    }
}

#[tokio::test]
async fn write_into_a_read_only_config_directory_is_live_but_reported_unsaved() {
    let Some(fixture) = Fixture::new() else {
        return;
    };

    let response = fixture
        .put("audio.device", &serde_json::json!("microphone"))
        .await;

    assert_eq!(response["data"]["key"], "audio.device");
    assert_eq!(response["data"]["value"], "microphone");
    assert_eq!(
        response["data"]["persisted"], false,
        "a write that never reached disk must not report success: {response}"
    );
    let error = response["data"]["persistence_error"]
        .as_str()
        .expect("an unsaved write should carry its persistence error");
    assert!(
        error.contains(CONFIG_FILE_NAME),
        "the error should name the config file: {error}"
    );
    assert_eq!(
        fixture.manager.get().audio.device,
        "microphone",
        "the live config stays authoritative"
    );
    assert!(
        !fixture.config_path.exists(),
        "the read-only directory kept the file from being written"
    );

    fixture.unlock_and_converge();
    assert_eq!(
        ConfigManager::load(&fixture.config_path)
            .expect("converged config should load")
            .audio
            .device,
        "microphone"
    );
}

#[tokio::test]
async fn diagnose_warns_while_the_config_file_lags_and_clears_after_retry() {
    let Some(fixture) = Fixture::new() else {
        return;
    };
    let check = fixture.persistence_check().await;
    assert_eq!(check["status"], "pass", "nothing lags yet: {check}");

    let response = fixture
        .put("audio.device", &serde_json::json!("microphone"))
        .await;
    let error = response["data"]["persistence_error"]
        .as_str()
        .expect("an unsaved write should carry its persistence error")
        .to_owned();

    let check = fixture.persistence_check().await;
    assert_eq!(check["category"], "config");
    assert_eq!(check["status"], "warning", "the file lags: {check}");
    let detail = check["detail"].as_str().expect("check detail is a string");
    assert!(
        detail.contains(CONFIG_FILE_NAME),
        "the warning should name the config file: {detail}"
    );
    assert!(
        detail.contains(&error),
        "the warning should carry the last write error: {detail}"
    );

    fixture.unlock_and_converge();

    let check = fixture.persistence_check().await;
    assert_eq!(
        check["status"], "pass",
        "a landed retry clears the warning: {check}"
    );
}

#[tokio::test]
async fn unchanged_and_reset_writes_report_the_lag_until_a_write_lands() {
    let Some(fixture) = Fixture::new() else {
        return;
    };

    let first = fixture
        .put("audio.device", &serde_json::json!("microphone"))
        .await;
    assert_eq!(first["data"]["persisted"], false);

    let unchanged = fixture
        .put("audio.device", &serde_json::json!("microphone"))
        .await;
    assert_eq!(
        unchanged["data"]["persisted"], false,
        "repeating the live value does not make the file catch up: {unchanged}"
    );
    assert!(unchanged["data"]["persistence_error"].is_string());

    let reset = fixture.delete("audio.device").await;
    assert_eq!(reset["data"]["value"], "default");
    assert_eq!(
        reset["data"]["persisted"], false,
        "a reset into a read-only directory is unsaved too: {reset}"
    );

    fixture.unlock_and_converge();

    let saved = fixture
        .put("audio.device", &serde_json::json!("line-in"))
        .await;
    assert_eq!(saved["data"]["persisted"], true, "{saved}");
    assert!(
        saved["data"].get("persistence_error").is_none(),
        "a saved write carries no error: {saved}"
    );
    assert_eq!(
        ConfigManager::load(&fixture.config_path)
            .expect("saved config should load")
            .audio
            .device,
        "line-in"
    );
}
