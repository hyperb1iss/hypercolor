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
    assert!(body["paths"]["/api/v1/output"]["get"].is_object());
    assert!(body["paths"]["/api/v1/output"]["patch"].is_object());
    assert_eq!(
        body["paths"]["/api/v1/output"]["patch"]["requestBody"]["content"]["application/json"]["schema"]
            ["$ref"],
        "#/components/schemas/OutputPatchRequest"
    );
    assert!(body["components"]["schemas"]["OutputResource"].is_object());
    assert!(body["paths"]["/api/v1/system/audio-devices"]["get"].is_object());
    // The merged routes carry no catalog entry either.
    for retired in [
        "/api/v1/output/power",
        "/api/v1/settings/brightness",
        "/api/v1/audio/devices",
        "/api/v1/effects/pause",
        "/api/v1/effects/resume",
    ] {
        assert!(
            body["paths"][retired].is_null(),
            "{retired} must be absent from the catalog"
        );
    }
    for retired in [
        "SetOutputPowerRequest",
        "OutputPowerResponse",
        "OutputPowerStatus",
        "SetBrightnessRequest",
        "PauseEffectResponse",
        "ResumeEffectResponse",
    ] {
        assert!(
            body["components"]["schemas"][retired].is_null(),
            "{retired} must be absent from the schema catalog"
        );
    }
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
