//! The persistent audit trail of state-changing requests.

use std::fs;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::Path;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::ConnectInfo;
use http::{Request, StatusCode};
use hypercolor_daemon::api;
use hypercolor_daemon::app_state::AppStateBuilder;
use hypercolor_daemon::audit_log::{self, AUDIT_LOG_FILE, AuditLog, AuditPeer, RequestLine};
use hypercolor_daemon::state_history::{StoreRoots, daemon_store_files};
use hypercolor_types::api::system::{AuditEntry, AuditTransport};
use serde_json::Value;
use tower::ServiceExt;

fn sample(index: usize) -> AuditEntry {
    audit_log::entry(
        None,
        AuditTransport::Http,
        RequestLine {
            method: "POST",
            path: &format!("/api/v1/sample/{index}"),
            tool: None,
            peer: &AuditPeer::new("127.0.0.1".to_owned(), None, "audit-test"),
        },
        200,
        &[],
        1.0,
    )
}

#[test]
fn the_trail_rotates_and_never_outgrows_its_files() {
    let directory = tempfile::tempdir().expect("tempdir");
    let logs = directory.path().join("logs");
    let log = AuditLog::new(logs.clone(), &[]).with_limits(2048, 2);
    for index in 0..200 {
        log.append(&sample(index)).expect("append");
    }

    let mut files: Vec<_> = fs::read_dir(&logs)
        .expect("logs dir")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .into_string()
                .expect("utf-8")
        })
        .collect();
    files.sort();
    assert_eq!(
        files,
        ["api-audit.1.jsonl", "api-audit.2.jsonl", "api-audit.jsonl"]
    );
    for file in &files {
        let size = fs::metadata(logs.join(file)).expect("metadata").len();
        assert!(size <= 2048, "{file} is {size} bytes");
    }

    let recent = log.recent(5).expect("recent");
    let paths: Vec<_> = recent.iter().map(|entry| entry.path.as_str()).collect();
    assert_eq!(
        paths,
        [
            "/api/v1/sample/199",
            "/api/v1/sample/198",
            "/api/v1/sample/197",
            "/api/v1/sample/196",
            "/api/v1/sample/195"
        ]
    );
    let everything = log.recent(1000).expect("recent");
    assert!(everything.len() < 200, "old entries rotated away");
    assert!(
        everything
            .windows(2)
            .all(|pair| index_of(&pair[0]) > index_of(&pair[1])),
        "newest first across rotated files"
    );
}

fn index_of(entry: &AuditEntry) -> usize {
    entry
        .path
        .rsplit('/')
        .next()
        .and_then(|index| index.parse().ok())
        .expect("sample path ends in its index")
}

fn request(method: &str, uri: &str, body: Option<&str>) -> Request<Body> {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header("user-agent", "audit-test/1.0")
        .header("authorization", "Bearer header-secret")
        .header("x-forwarded-for", "203.0.113.9")
        .header("content-type", "application/json")
        .body(body.map_or_else(Body::empty, |body| Body::from(body.to_owned())))
        .expect("request builds");
    request.extensions_mut().insert(ConnectInfo(SocketAddr::new(
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        50_000,
    )));
    request
}

async fn body_json(response: axum::response::Response) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    serde_json::from_slice(&bytes).expect("json body")
}

fn audit_state(directory: &Path) -> (Arc<hypercolor_daemon::app_state::AppState>, Arc<AuditLog>) {
    let data_dir = directory.join("data");
    fs::create_dir_all(&data_dir).expect("data dir");
    let mut state = AppStateBuilder::new(data_dir).build();
    let roots = StoreRoots {
        config_file: directory.join("hypercolor.toml"),
        config_dir: directory.to_path_buf(),
        data_dir: state.data_dir.clone(),
        state_dir: state.state_dir.clone(),
    };
    audit_log::install_store_observer();
    let log = Arc::new(AuditLog::new(
        directory.join("logs"),
        &daemon_store_files(&roots),
    ));
    state.audit_log = Some(Arc::clone(&log));
    (Arc::new(state), log)
}

#[tokio::test]
async fn only_mutating_requests_are_audited_and_no_secret_reaches_the_trail() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (state, log) = audit_state(directory.path());
    let app = api::build_router(state, None);

    let listed = app
        .clone()
        .oneshot(request("GET", "/api/v1/scenes?token=query-secret", None))
        .await
        .expect("list scenes");
    assert_eq!(listed.status(), StatusCode::OK);
    assert!(
        log.recent(10).expect("recent").is_empty(),
        "reads are not audited"
    );

    let created = app
        .clone()
        .oneshot(request(
            "POST",
            "/api/v1/scenes?token=query-secret",
            Some(r#"{"name": "Audit Watch", "description": "body-secret"}"#),
        ))
        .await
        .expect("create scene");
    assert_eq!(created.status(), StatusCode::CREATED);
    let scene_id = body_json(created).await["data"]["id"]
        .as_str()
        .expect("scene id")
        .to_owned();

    for (method, uri) in [
        ("PUT", format!("/api/v1/scenes/{scene_id}")),
        ("PATCH", "/api/v1/scene".to_owned()),
        ("DELETE", format!("/api/v1/scenes/{scene_id}")),
    ] {
        app.clone()
            .oneshot(request(method, &uri, Some(r#"{"note": "body-secret"}"#)))
            .await
            .expect("mutation runs");
    }

    let entries = log.recent(10).expect("recent");
    let methods: Vec<_> = entries.iter().map(|entry| entry.method.as_str()).collect();
    assert_eq!(methods, ["DELETE", "PATCH", "PUT", "POST"]);

    let create = &entries[3];
    assert_eq!(create.transport, AuditTransport::Http);
    assert_eq!(create.path, "/api/v1/scenes");
    assert_eq!(create.status, 201);
    assert_eq!(create.remote, "127.0.0.1", "the socket peer, not a header");
    assert_eq!(create.forwarded_for.as_deref(), Some("203.0.113.9"));
    assert_eq!(create.user_agent, "audit-test/1.0");
    assert!(
        create.stores.iter().any(|store| store == "scenes"),
        "creating a scene changes the scene store: {:?}",
        create.stores
    );
    let delete = &entries[0];
    assert!(delete.stores.iter().any(|store| store == "scenes"));

    let trail =
        fs::read_to_string(directory.path().join("logs").join(AUDIT_LOG_FILE)).expect("trail file");
    for secret in [
        "query-secret",
        "header-secret",
        "body-secret",
        "token=",
        "Bearer",
    ] {
        assert!(!trail.contains(secret), "{secret} leaked into the trail");
    }
}

#[tokio::test]
async fn layout_writes_on_workflow_tasks_are_attributed() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (state, log) = audit_state(directory.path());
    let app = api::build_router(state, None);

    let created = app
        .clone()
        .oneshot(request(
            "POST",
            "/api/v1/layouts",
            Some(r#"{"name": "Audit Layout"}"#),
        ))
        .await
        .expect("create layout");
    assert!(created.status().is_success(), "{}", created.status());
    let layout_id = body_json(created).await["data"]["id"]
        .as_str()
        .expect("layout id")
        .to_owned();
    let deleted = app
        .clone()
        .oneshot(request(
            "DELETE",
            &format!("/api/v1/layouts/{layout_id}"),
            None,
        ))
        .await
        .expect("delete layout");
    assert!(deleted.status().is_success(), "{}", deleted.status());

    let entries = log.recent(10).expect("recent");
    assert_eq!(entries.len(), 2);
    for entry in &entries {
        assert!(
            entry.stores.iter().any(|store| store == "layouts"),
            "{} {} should name the layout store: {:?}",
            entry.method,
            entry.path,
            entry.stores
        );
    }
}

#[tokio::test]
async fn the_audit_endpoint_returns_the_newest_entries_and_is_not_itself_audited() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (state, log) = audit_state(directory.path());
    let app = api::build_router(state, None);
    for name in ["First", "Second", "Third"] {
        app.clone()
            .oneshot(request(
                "POST",
                "/api/v1/scenes",
                Some(&format!(r#"{{"name": "{name}"}}"#)),
            ))
            .await
            .expect("create scene");
    }

    let response = app
        .clone()
        .oneshot(request("GET", "/api/v1/system/audit?limit=2", None))
        .await
        .expect("audit query");
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    let items = body["data"]["items"].as_array().expect("items");
    assert_eq!(items.len(), 2);
    assert_eq!(body["data"]["total"], 2);
    let newest: AuditEntry = serde_json::from_value(items[0].clone()).expect("entry");
    assert_eq!(newest.method, "POST");
    assert_eq!(log.recent(10).expect("recent").len(), 3);
}

#[tokio::test]
async fn store_writes_on_blocking_threads_stay_attributed() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("blocking.json");
    audit_log::install_store_observer();

    let ((), changed) = audit_log::collect_changes(async {
        let scope = audit_log::current_change_scope();
        let path = path.clone();
        tokio::task::spawn_blocking(move || {
            audit_log::with_change_scope(scope, || {
                hypercolor_daemon::persistence::write_atomic(&path, b"written off-task")
                    .expect("write");
            });
        })
        .await
        .expect("blocking task");
    })
    .await;
    assert_eq!(changed.len(), 1);
    assert_eq!(changed[0].file_name(), path.file_name());

    let ((), unscoped) = audit_log::collect_changes(async {
        let path = path.clone();
        tokio::task::spawn_blocking(move || {
            hypercolor_daemon::persistence::write_atomic(&path, b"a background write")
                .expect("write");
        })
        .await
        .expect("blocking task");
    })
    .await;
    assert!(
        unscoped.is_empty(),
        "writes outside the scope are not attributed"
    );
}
