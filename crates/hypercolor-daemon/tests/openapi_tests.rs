use std::collections::BTreeSet;
use std::sync::{Arc, LazyLock, Mutex};

use axum::body::Body;
use http::{Request, StatusCode};
use hypercolor_core::config::ConfigManager;
use hypercolor_daemon::api::openapi::ROUTES;
use hypercolor_daemon::api::{self, AppState};
use tower::ServiceExt;

static DATA_DIR_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

fn isolated_state() -> AppState {
    let _lock = DATA_DIR_LOCK
        .lock()
        .expect("data dir lock should not be poisoned");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let data_dir = tempdir.path().join("data");
    std::fs::create_dir_all(&data_dir).expect("temp data dir should be created");
    ConfigManager::set_data_dir_override(Some(data_dir));
    let state = AppState::new();
    ConfigManager::set_data_dir_override(None);
    state
}

fn test_app() -> axum::Router {
    api::build_router(Arc::new(isolated_state()), None)
}

async fn body_json(response: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("failed to read response body");
    serde_json::from_slice(&bytes).expect("failed to parse JSON body")
}

async fn body_text(response: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("failed to read response body");
    String::from_utf8(bytes.to_vec()).expect("failed to decode UTF-8 body")
}

#[tokio::test]
async fn openapi_json_is_served_with_expected_paths() {
    let app = test_app();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/openapi.json")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("openapi request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["openapi"], "3.1.0");
    assert!(body["paths"]["/health"].is_object());
    assert!(body["paths"]["/api/v1/status"].is_object());
    assert!(body["paths"]["/api/v1/devices"].is_object());
    assert!(body["paths"]["/api/v1/effects"].is_object());
    assert!(body["paths"]["/api/v1/output/power"]["get"].is_object());
    assert!(body["paths"]["/api/v1/output/power"]["put"].is_object());
    assert_eq!(
        body["paths"]["/api/v1/output/power"]["put"]["requestBody"]["content"]["application/json"]
            ["schema"]["$ref"],
        "#/components/schemas/SetOutputPowerRequest"
    );
    assert!(body["components"]["schemas"]["OutputPowerResponse"].is_object());
    assert_eq!(
        body["paths"]["/api/v1/effects/pause"]["post"]["responses"]["200"]["content"]["application/json"]
            ["schema"]["$ref"],
        "#/components/schemas/ApiResponse_PauseEffectResponse"
    );
    assert_eq!(
        body["paths"]["/api/v1/effects/resume"]["post"]["responses"]["200"]["content"]["application/json"]
            ["schema"]["$ref"],
        "#/components/schemas/ApiResponse_ResumeEffectResponse"
    );
    assert!(body["paths"]["/api/v1/effects/{id}/apply"].is_object());
    assert_ne!(
        body["paths"]["/api/v1/effects/{id}/apply"]["post"]["requestBody"]["required"],
        true
    );
    assert_eq!(
        body["paths"]["/api/v1/scenes/{id}/zones"]["post"]["requestBody"]["required"],
        true
    );
    assert_eq!(
        body["paths"]["/api/v1/scenes/{id}/zones/{zone_id}/layout"]["put"]["requestBody"]["content"]
            ["application/json"]["schema"]["$ref"],
        "#/components/schemas/SpatialLayout"
    );
    assert!(body["components"]["schemas"]["SpatialLayout"].is_object());
    assert!(
        body["paths"]["/api/v1/scenes/{id}/unassigned-behavior"]["patch"]["responses"]["412"]
            .is_object()
    );
    assert!(body["paths"]["/api/v1/control-surfaces"].is_object());
    assert!(body["components"]["schemas"]["ControlSurfaceDocument"].is_object());
    assert!(body["components"]["schemas"]["ApplyControlChangesRequest"].is_object());
    assert!(body["components"]["schemas"]["ControlFieldDescriptor"].is_object());
    assert!(body["components"]["schemas"]["CreateZoneRequest"].is_object());
    assert!(body["components"]["schemas"]["AssignDevicesRequest"].is_object());
    let input_status = &body["components"]["schemas"]["InputStatus"];
    assert!(input_status.is_object());
    for legacy_field in [
        "enabled",
        "host_capture_registered",
        "host_capturing",
        "devices_opened",
        "devices_denied",
        "degraded",
        "backends",
    ] {
        assert!(
            input_status["properties"][legacy_field].is_object(),
            "missing legacy InputStatus field {legacy_field}"
        );
    }
    assert_eq!(
        input_status["properties"]["sources"]["items"]["$ref"],
        "#/components/schemas/InputSourceStatus"
    );
    let source_status = &body["components"]["schemas"]["InputSourceStatus"];
    assert_eq!(source_status["properties"]["freshness"]["type"], "string");
    assert!(source_status["properties"]["source_graph_generation"].is_object());
    assert!(source_status["properties"]["session_generation"].is_object());
    assert!(source_status["properties"]["last_sample_age_ms"].is_object());
    assert!(source_status["properties"]["freshness_remaining_ms"].is_object());
    assert!(source_status["properties"]["denied_resource_count"].is_object());
    assert!(body["components"]["schemas"]["InputSourceIssueStatus"].is_object());
    for (path, method) in [
        ("/api/v1/input/authorize", "post"),
        ("/api/v1/capture/authorize", "post"),
        ("/api/v1/capture/source/pick", "post"),
        ("/api/v1/capture/monitors", "get"),
    ] {
        assert!(
            body["paths"][path][method].is_object(),
            "missing capture operation {} {path}",
            method.to_uppercase()
        );
        assert_eq!(
            body["paths"][path][method]["responses"]["403"]["content"]["application/json"]["schema"]
                ["$ref"],
            "#/components/schemas/ApiErrorResponse"
        );
    }
    assert!(body["components"]["schemas"]["CaptureAuthorizationResponse"].is_object());
    assert!(body["components"]["schemas"]["CapturePickerResponse"].is_object());
    assert!(body["components"]["schemas"]["CaptureMonitor"].is_object());
    assert!(body["components"]["schemas"]["ProtectedSourceGrantOwner"].is_object());

    for route in ROUTES {
        let operation = &body["paths"][route.path][route.method];
        assert!(
            operation.is_object(),
            "missing OpenAPI operation {} {}",
            route.method.to_uppercase(),
            route.path
        );
        assert_eq!(
            operation["operationId"],
            route.operation_id,
            "unexpected operationId for {} {}",
            route.method.to_uppercase(),
            route.path
        );
    }
}

fn balanced_call(input: &str) -> &str {
    let mut depth = 0_usize;
    let mut in_string = false;
    let mut escaped = false;
    let mut saw_open = false;

    for (index, character) in input.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }
        match character {
            '"' => in_string = true,
            '(' => {
                saw_open = true;
                depth += 1;
            }
            ')' => {
                depth -= 1;
                if saw_open && depth == 0 {
                    return &input[..=index];
                }
            }
            _ => {}
        }
    }

    panic!("unterminated router call: {input}");
}

fn quoted_path(call: &str) -> &str {
    let start = call.find('"').expect("router call should contain a path") + 1;
    let end = call[start..]
        .find('"')
        .expect("router path should have a closing quote");
    &call[start..start + end]
}

fn router_operations() -> BTreeSet<(String, String)> {
    let source = include_str!("../src/api/mod.rs");
    let mut router = source
        .split_once("let api = Router::new()")
        .expect("router construction should be present")
        .1
        .split_once("let mut api = api;")
        .expect("router construction should have a stable boundary")
        .0;
    let mut operations = BTreeSet::new();

    while let Some(index) = router.find(".route(") {
        let call = balanced_call(&router[index..]);
        let path = format!("/api/v1{}", quoted_path(call));
        for method in ["get", "post", "put", "patch", "delete"] {
            if call.contains(&format!("axum::routing::{method}("))
                || call.contains(&format!(".{method}("))
            {
                operations.insert((method.to_owned(), path.clone()));
            }
        }
        router = &router[index + call.len()..];
    }

    let screenshot_index = source
        .find(".nest_service(")
        .expect("effect screenshot service should be mounted");
    let screenshot_service = balanced_call(&source[screenshot_index..]);
    operations.insert((
        "get".to_owned(),
        format!("/api/v1{}", quoted_path(screenshot_service)),
    ));
    operations
}

#[test]
fn every_static_router_operation_is_cataloged() {
    let catalog = ROUTES
        .iter()
        .map(|route| (route.method.to_owned(), route.path.to_owned()))
        .collect::<BTreeSet<_>>();
    let missing = router_operations()
        .difference(&catalog)
        .cloned()
        .collect::<Vec<_>>();

    assert!(
        missing.is_empty(),
        "router operations missing from OpenAPI catalog: {missing:?}"
    );
}

#[test]
fn route_catalog_operation_ids_are_unique() {
    let mut operation_ids = BTreeSet::new();
    for route in ROUTES {
        assert!(
            operation_ids.insert(route.operation_id),
            "duplicate OpenAPI operationId {}",
            route.operation_id
        );
    }
}

#[tokio::test]
async fn swagger_ui_is_served() {
    let app = test_app();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/docs/")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("swagger ui request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let content_type = response
        .headers()
        .get(http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let body = body_text(response).await;
    assert!(content_type.starts_with("text/html"));
    assert!(!body.is_empty());
}
