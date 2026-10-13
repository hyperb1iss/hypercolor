//! `GET /api/v1/effects/console`: cursors, effect and severity filters, and
//! catalog id mapping over the process-wide effect console journal.
//!
//! The journal is process-wide, so every test records under its own effect
//! name and reads from a cursor it took first.

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use http::{Request, StatusCode};
use hypercolor_core::effect::console::{EffectConsoleJournal, EffectConsoleRecord};
use hypercolor_daemon::api;
use hypercolor_daemon::app_state::AppState;
use hypercolor_types::api::effects::EffectConsoleResponse;
use serde_json::Value;
use tower::ServiceExt;

const PROBE_HTML: &str = r#"<!DOCTYPE html><html><head><title>Console Probe</title></head><body><canvas id="exCanvas"></canvas><script>console.log('hi')</script></body></html>"#;

fn isolated_state() -> (Arc<AppState>, tempfile::TempDir) {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let data_dir = tempdir.path().join("data");
    std::fs::create_dir_all(&data_dir).expect("temp data dir should be created");
    (Arc::new(AppState::new_with_data_dir(data_dir)), tempdir)
}

fn record(effect: &str, source_path: PathBuf, level: &str, message: &str, line: Option<usize>) {
    EffectConsoleJournal::global().record(EffectConsoleRecord {
        level: level.to_owned(),
        message: message.to_owned(),
        effect_name: effect.to_owned(),
        source_path,
        line,
    });
}

async fn get(state: &Arc<AppState>, uri: &str) -> (StatusCode, Value) {
    let response = api::build_router(Arc::clone(state), None)
        .oneshot(
            Request::get(uri)
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("route should respond");
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should read");
    let json = serde_json::from_slice(&body).expect("body should be JSON");
    (status, json)
}

async fn read_console(state: &Arc<AppState>, query: &str) -> EffectConsoleResponse {
    let (status, json) = get(state, &format!("/api/v1/effects/console?{query}")).await;
    assert_eq!(status, StatusCode::OK, "{json}");
    serde_json::from_value(json["data"].clone()).expect("console response should decode")
}

#[tokio::test]
async fn console_filters_by_effect_and_severity_after_a_cursor() {
    let (state, _tempdir) = isolated_state();
    let mark = EffectConsoleJournal::global().latest_seq();
    let probe = "console-api-filter-probe";
    record(probe, "/fx/probe.html".into(), "log", "tick", None);
    record(probe, "/fx/probe.html".into(), "warn", "slow frame", None);
    record(probe, "/fx/probe.html".into(), "error", "boom", Some(12));
    record(
        "console-api-bystander",
        "/fx/other.html".into(),
        "error",
        "not mine",
        None,
    );

    let warnings = read_console(&state, &format!("after={mark}&effect={probe}&level=warn")).await;
    assert_eq!(
        warnings
            .entries
            .iter()
            .map(|entry| entry.message.as_str())
            .collect::<Vec<_>>(),
        ["slow frame", "boom"]
    );
    assert_eq!(warnings.entries[1].line, Some(12));
    assert_eq!(warnings.entries[1].source_file, "probe.html");
    assert!(
        warnings.entries[0].effect_id.is_none(),
        "probe is not installed"
    );
    assert!(!warnings.truncated);

    let everything = read_console(&state, &format!("after={mark}&effect={probe}")).await;
    assert_eq!(everything.entries.len(), 3, "debug is the default floor");

    let newer = read_console(
        &state,
        &format!("after={}&effect={probe}", everything.next_seq),
    )
    .await;
    assert!(newer.entries.is_empty(), "the cursor skips what was read");
}

#[tokio::test]
async fn console_maps_installed_effects_to_their_catalog_id() {
    let (state, _tempdir) = isolated_state();
    let boundary = "console-probe-boundary";
    let upload = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"console-probe.html\"\r\nContent-Type: text/html\r\n\r\n{PROBE_HTML}\r\n--{boundary}--\r\n"
    );
    let installed = api::build_router(Arc::clone(&state), None)
        .oneshot(
            Request::post("/api/v1/effects/install")
                .header(
                    http::header::CONTENT_TYPE,
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(upload))
                .expect("request should build"),
        )
        .await
        .expect("install should respond");
    assert_eq!(installed.status(), StatusCode::CREATED);
    let installed: Value = serde_json::from_slice(
        &axum::body::to_bytes(installed.into_body(), usize::MAX)
            .await
            .expect("body should read"),
    )
    .expect("install body should be JSON");
    assert!(
        installed["data"]["console_after"].is_u64(),
        "installs report the console cursor taken before the page loads"
    );
    let id = installed["data"]["id"].as_str().expect("id").to_owned();
    let path = PathBuf::from(installed["data"]["path"].as_str().expect("path"));

    let mark = EffectConsoleJournal::global().latest_seq();
    record("Console Probe", path, "error", "ReferenceError: x", Some(3));

    let page = read_console(&state, &format!("after={mark}&effect={id}")).await;
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.entries[0].effect_id.as_deref(), Some(id.as_str()));
    assert_eq!(page.entries[0].source_file, "console-probe.html");
}

#[tokio::test]
async fn console_rejects_out_of_range_limits_and_unknown_levels() {
    let (state, _tempdir) = isolated_state();

    let (status, json) = get(&state, "/api/v1/effects/console?limit=0").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(json["error"]["details"]["field"], "limit");

    let (status, _) = get(&state, "/api/v1/effects/console?level=loud").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}
