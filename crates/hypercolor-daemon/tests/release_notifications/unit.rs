use super::*;

#[test]
fn compares_semantic_versions_without_downgrades_or_previews() {
    assert!(newer("v0.10.0", "0.9.0"));
    assert!(!newer("v0.9.0", "0.10.0"));
    assert!(!newer("v0.10.0-rc.1", "0.9.0"));
    assert!(!newer("v0.10.0", "0.10.0"));
    assert!(!newer("broken", "0.10.0"));
    assert!(!newer("v0.10.0+new", "0.10.0+old"));
    assert!(!newer("v0.10.0+build", "0.10.0"));
    assert!(newer("v0.10.0", "0.10.0-rc.1"));
}

fn release() -> GithubRelease {
    GithubRelease {
        tag_name: "v99.0.0".into(),
        name: None,
        body: Some("notes".into()),
        html_url: "https://github.com/hyperb1iss/hypercolor/releases/tag/v99.0.0".into(),
        draft: false,
        prerelease: false,
        assets: vec![GithubAsset {
            name: "Hypercolor_99.0.0_x64-setup.exe".into(),
            browser_download_url:
                "https://github.com/hyperb1iss/hypercolor/releases/download/v99.0.0/setup.exe"
                    .into(),
        }],
    }
}

#[test]
fn chooses_exact_asset_and_falls_back_for_unknown_installation() {
    let windows = release()
        .into_update("0.1.0", "windows", "x86_64")
        .expect("parse")
        .expect("update");
    assert!(windows.download_url.is_some());
    assert_eq!(windows.version, "99.0.0");
    assert_eq!(windows.notes, "notes");
    let linux = release()
        .into_update("0.1.0", "linux", "x86_64")
        .expect("parse")
        .expect("update");
    assert!(linux.download_url.is_none());
    let missing = release()
        .into_update("0.1.0", "macos", "aarch64")
        .expect("parse")
        .expect("update");
    assert!(missing.download_url.is_none());
}

#[test]
fn rejects_drafts_previews_and_untrusted_links() {
    let mut draft = release();
    draft.draft = true;
    assert!(
        draft
            .into_update("0.1.0", "windows", "x86_64")
            .expect("parse")
            .is_none()
    );
    let mut preview = release();
    preview.prerelease = true;
    assert!(
        preview
            .into_update("0.1.0", "windows", "x86_64")
            .expect("parse")
            .is_none()
    );
    let mut bad = release();
    bad.html_url = "https://example.com".into();
    assert!(bad.into_update("0.1.0", "windows", "x86_64").is_err());
    let mut bad_asset = release();
    bad_asset.assets[0].browser_download_url = "javascript:alert(1)".into();
    assert!(
        bad_asset
            .into_update("0.1.0", "windows", "x86_64")
            .expect("parse")
            .expect("update")
            .download_url
            .is_none()
    );
}

#[tokio::test]
async fn persists_preferences_and_cached_release_and_emits_events() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("notifications.json");
    let bus = Arc::new(HypercolorBus::new());
    let mut events = bus.subscribe_all();
    let service = ReleaseNotifications::open(&path, Arc::clone(&bus)).expect("service");
    service
        .patch(UpdatePreferencesPatch {
            automatic_checks: Some(false),
            skipped_version: Some("99.0.0".into()),
            ..Default::default()
        })
        .await
        .expect("preferences");
    assert!(matches!(events.try_recv().expect("preference event").event,
        HypercolorEvent::ExtensionStateChanged { source, kind, .. }
        if source == "oss-updates" && kind == "status"));
    let restored = ReleaseNotifications::open(&path, bus).expect("restore");
    let status = restored.status().await;
    assert!(!status.automatic_checks);
    assert_eq!(status.skipped_version.as_deref(), Some("99.0.0"));
    assert!(
        restored
            .patch(UpdatePreferencesPatch {
                skipped_version: Some("99.0.0".into()),
                clear_skipped_version: true,
                ..Default::default()
            })
            .await
            .is_err()
    );
    restored
        .patch(UpdatePreferencesPatch {
            clear_skipped_version: true,
            ..Default::default()
        })
        .await
        .expect("clear");
    assert!(restored.status().await.skipped_version.is_none());
}

#[test]
fn disabled_registry_has_no_release_capability() {
    let registry = crate::extensions::ExtensionRegistry::default();
    assert!(!registry.contains::<ReleaseNotifications>());
}

#[tokio::test]
async fn successful_check_caches_release_and_failure_preserves_it() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("state.json");
    let service =
        ReleaseNotifications::open(&path, Arc::new(HypercolorBus::new())).expect("service");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    let address = listener.local_addr().expect("address");
    let router = axum::Router::new()
        .route(
            "/success",
            axum::routing::get(|| async {
                Json(serde_json::json!({
                    "tag_name": "v99.0.0", "name": "New release", "body": "release notes",
                    "html_url": "https://github.com/hyperb1iss/hypercolor/releases/tag/v99.0.0",
                    "draft": false, "prerelease": false, "assets": []
                }))
            }),
        )
        .route(
            "/failure",
            axum::routing::get(|| async { axum::http::StatusCode::SERVICE_UNAVAILABLE }),
        )
        .route(
            "/rate-limited",
            axum::routing::get(|| async { axum::http::StatusCode::TOO_MANY_REQUESTS }),
        )
        .route("/malformed", axum::routing::get(|| async { "not json" }));
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.expect("server");
    });
    let checked = service
        .check_url(&format!("http://{address}/success"))
        .await
        .expect("check");
    assert!(checked.last_checked.is_some());
    assert_eq!(checked.release.as_ref().expect("release").version, "99.0.0");
    for endpoint in ["failure", "rate-limited", "malformed"] {
        assert!(
            service
                .check_url(&format!("http://{address}/{endpoint}"))
                .await
                .is_err()
        );
        assert_eq!(service.status().await, checked);
    }
    let restored =
        ReleaseNotifications::open(&path, Arc::new(HypercolorBus::new())).expect("restore");
    assert_eq!(restored.status().await, checked);
    server.abort();
}

#[test]
fn selects_linux_archive_for_exact_architecture() {
    let mut linux = release();
    linux.assets = ["amd64", "arm64"].into_iter().map(|arch| GithubAsset {
        name: format!("hypercolor-99.0.0-linux-{arch}.tar.gz"),
        browser_download_url: format!("https://github.com/hyperb1iss/hypercolor/releases/download/v99.0.0/hypercolor-99.0.0-linux-{arch}.tar.gz"),
    }).collect();
    let selected = linux
        .into_update("0.1.0", "linux", "aarch64")
        .expect("parse")
        .expect("release");
    assert!(
        selected
            .download_url
            .expect("download")
            .ends_with("-arm64.tar.gz")
    );
}

#[tokio::test]
async fn registration_controls_routes_capability_and_preference_mutations() {
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;

    for enabled in [false, true] {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("notifications.json");
        let mut state = AppState::new_with_data_dir(directory.path().to_path_buf());
        if enabled {
            let service = Arc::new(
                ReleaseNotifications::open(&path, Arc::clone(&state.event_bus)).expect("service"),
            );
            state
                .extensions
                .insert(Arc::clone(&service))
                .expect("register");
            state.api_extensions.push(service);
        }
        let app = crate::api::build_router(Arc::new(state), None);
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/openapi.json")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("schema response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("schema body");
        let schema: serde_json::Value = serde_json::from_slice(&body).expect("schema");
        assert_eq!(
            schema["paths"]
                .get("/api/v1/release-notifications")
                .is_some(),
            enabled
        );
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/release-notifications")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(
            response.status(),
            if enabled {
                StatusCode::OK
            } else {
                StatusCode::NOT_FOUND
            }
        );
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/system")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let status: serde_json::Value = serde_json::from_slice(&body).expect("status");
        assert_eq!(
            status["data"]["status"]["capabilities"]
                .as_array()
                .expect("capabilities")
                .iter()
                .any(|value| value == "oss-release-notifications"),
            enabled
        );
        let response = app
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri("/api/v1/release-notifications")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"automatic_checks":false,"skipped_version":"99.0.0"}"#,
                    ))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(
            response.status(),
            if enabled {
                StatusCode::OK
            } else {
                StatusCode::NOT_FOUND
            }
        );
        if enabled {
            let restored =
                ReleaseNotifications::open(&path, Arc::new(HypercolorBus::new())).expect("restore");
            let status = restored.status().await;
            assert!(!status.automatic_checks);
            assert_eq!(status.skipped_version.as_deref(), Some("99.0.0"));
        } else {
            assert!(!path.exists());
        }
    }
}

#[tokio::test(start_paused = true)]
async fn background_worker_honors_disabled_preference_and_cancellation() {
    let directory = tempfile::tempdir().expect("tempdir");
    let service = Arc::new(
        ReleaseNotifications::open(
            &directory.path().join("notifications.json"),
            Arc::new(HypercolorBus::new()),
        )
        .expect("service"),
    );
    service.saved.lock().await.automatic_checks = false;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    let url = format!("http://{}", listener.local_addr().expect("address"));
    let runner = Arc::clone(&service);
    let worker = tokio::spawn(async move {
        runner.run_with_url(&url, FAILURE_RETRY_INTERVAL).await;
    });
    tokio::task::yield_now().await;
    tokio::time::advance(CHECK_INTERVAL * 2).await;
    tokio::task::yield_now().await;
    assert!(
        tokio::time::timeout(Duration::from_millis(1), listener.accept())
            .await
            .is_err()
    );
    assert!(service.status().await.last_checked.is_none());
    service.cancel.cancel();
    worker.await.expect("cancelled worker");
}

#[tokio::test(start_paused = true)]
async fn background_worker_keeps_fresh_cache_on_startup() {
    let directory = tempfile::tempdir().expect("tempdir");
    let service = Arc::new(
        ReleaseNotifications::open(
            &directory.path().join("notifications.json"),
            Arc::new(HypercolorBus::new()),
        )
        .expect("service"),
    );
    service.saved.lock().await.last_checked = Some(Utc::now());
    let before = service.status().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    let url = format!("http://{}", listener.local_addr().expect("address"));
    let runner = Arc::clone(&service);
    let worker = tokio::spawn(async move {
        runner.run_with_url(&url, FAILURE_RETRY_INTERVAL).await;
    });
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(3600)).await;
    tokio::task::yield_now().await;
    assert!(
        tokio::time::timeout(Duration::from_millis(1), listener.accept())
            .await
            .is_err()
    );
    assert_eq!(service.status().await, before);
    service.cancel.cancel();
    worker.await.expect("cancelled worker");
}

#[cfg(feature = "persistence-test-hooks")]
#[tokio::test]
async fn admitted_preferences_stay_live_when_disk_write_fails_and_retry_recovers() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("notifications.json");
    let bus = Arc::new(HypercolorBus::new());
    let mut events = bus.subscribe_all();
    let service = ReleaseNotifications::open(&path, bus).expect("service");
    service.writer.set_injected_replace_failures(usize::MAX);
    let accepted = service
        .patch(UpdatePreferencesPatch {
            automatic_checks: Some(false),
            skipped_version: Some("99.0.0".into()),
            ..Default::default()
        })
        .await
        .expect("admitted preference succeeds despite disk failure");
    assert!(!accepted.automatic_checks);
    assert_eq!(accepted.skipped_version.as_deref(), Some("99.0.0"));
    assert_eq!(service.status().await, accepted);
    assert!(events.try_recv().is_ok());
    assert!(
        !path.exists(),
        "injected replacement failure blocks disk visibility"
    );
    service.writer.set_injected_replace_failures(0);
    let writer = service.writer.clone();
    tokio::task::spawn_blocking(move || writer.flush(Duration::from_secs(5)))
        .await
        .expect("flush task")
        .expect("retry converges");
    let restored =
        ReleaseNotifications::open(&path, Arc::new(HypercolorBus::new())).expect("restore");
    assert_eq!(restored.status().await, accepted);
}

#[tokio::test]
async fn background_failure_recovers_on_retry_schedule() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let directory = tempfile::tempdir().expect("tempdir");
    let bus = Arc::new(HypercolorBus::new());
    let mut events = bus.subscribe_all();
    let service = Arc::new(
        ReleaseNotifications::open(&directory.path().join("notifications.json"), bus)
            .expect("service"),
    );
    let attempts = Arc::new(AtomicUsize::new(0));
    let counts = Arc::clone(&attempts);
    let router = axum::Router::new().route(
        "/",
        axum::routing::get(move || {
            let count = counts.fetch_add(1, Ordering::SeqCst);
            async move {
                if count == 0 {
                    axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response()
                } else {
                    Json(serde_json::json!({
                        "tag_name": "v99.0.0", "name": "Recovered release", "body": "notes",
                        "html_url": "https://github.com/hyperb1iss/hypercolor/releases/tag/v99.0.0",
                        "draft": false, "prerelease": false, "assets": []
                    }))
                    .into_response()
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    let url = format!("http://{}", listener.local_addr().expect("address"));
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.expect("server");
    });
    let runner = Arc::clone(&service);
    let started = tokio::time::Instant::now();
    let retry_interval = Duration::from_millis(10);
    let worker = tokio::spawn(async move {
        runner.run_with_url(&url, retry_interval).await;
    });
    tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .expect("offline recovery precedes daily interval")
        .expect("successful check event");
    assert!(started.elapsed() >= retry_interval);
    assert_eq!(FAILURE_RETRY_INTERVAL, Duration::from_mins(15));
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    assert_eq!(
        service.status().await.release.expect("release").version,
        "99.0.0"
    );
    service.cancel.cancel();
    worker.await.expect("worker shutdown");
    server.abort();
}
