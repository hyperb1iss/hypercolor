use std::collections::BTreeSet;
use std::sync::{Arc, LazyLock, Mutex};

use axum::body::Body;
use http::{Request, StatusCode};
use hypercolor_core::config::ConfigManager;
use hypercolor_daemon::api;
use hypercolor_daemon::app_state::AppState;
use tower::ServiceExt;

static DATA_DIR_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

fn openapi_document() -> serde_json::Value {
    serde_json::from_str(
        &hypercolor_daemon::api::openapi::document_json_pretty()
            .expect("OpenAPI document should serialize"),
    )
    .expect("OpenAPI document should parse")
}

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
    assert!(body["paths"]["/api/v1/system"].is_object());
    assert!(body["paths"]["/api/v1/devices"].is_object());
    assert!(body["paths"]["/api/v1/effects"].is_object());
    assert_eq!(
        body["components"]["schemas"]["EffectCategory"]["enum"],
        serde_json::json!([
            "ambient",
            "audio",
            "generative",
            "particle",
            "scenic",
            "interactive",
            "fun",
            "source",
            "utility",
            "display"
        ])
    );
    assert_eq!(
        body["components"]["schemas"]["EffectSourceKind"]["enum"],
        serde_json::json!(["native", "html", "shader"])
    );
    assert_eq!(
        body["components"]["schemas"]["EffectSummary"]["properties"]["category"]["$ref"],
        "#/components/schemas/EffectCategory"
    );
    assert_eq!(
        body["components"]["schemas"]["EffectSummary"]["properties"]["source"]["$ref"],
        "#/components/schemas/EffectSourceKind"
    );
    assert!(
        body["components"]["schemas"]["InstalledEffectResponse"]["properties"]["source"].is_null()
    );
    let effect_parameters = body["paths"]["/api/v1/effects"]["get"]["parameters"]
        .as_array()
        .expect("effect query parameters should be an array");
    for (name, schema) in [
        ("category", "EffectCategory"),
        ("source", "EffectSourceKind"),
    ] {
        let parameter = effect_parameters
            .iter()
            .find(|parameter| parameter["name"] == name)
            .unwrap_or_else(|| panic!("missing {name} effect filter"));
        assert_eq!(
            parameter["schema"]["oneOf"][1]["$ref"],
            format!("#/components/schemas/{schema}")
        );
    }
    assert!(body["paths"]["/api/v1/output"]["get"].is_object());
    assert!(body["paths"]["/api/v1/output"]["patch"].is_object());
    assert_eq!(
        body["paths"]["/api/v1/output"]["patch"]["requestBody"]["content"]["application/json"]["schema"]
            ["$ref"],
        "#/components/schemas/OutputPatchRequest"
    );
    assert!(body["components"]["schemas"]["OutputResource"].is_object());
    assert_eq!(
        body["paths"]["/api/v1/devices/{id}/attachments"]["put"]["requestBody"]["content"]["application/json"]
            ["schema"]["$ref"],
        "#/components/schemas/UpdateAttachmentsRequest"
    );
    assert_eq!(
        body["paths"]["/api/v1/devices/{id}/attachments"]["put"]["requestBody"]["required"],
        true
    );
    assert!(body["components"]["schemas"]["UpdateAttachmentsRequest"].is_object());
    assert!(body["paths"]["/api/v1/system/audio-devices"]["get"].is_object());
    // Retired routes stay absent from the runtime document.
    for retired in [
        "/api/v1/server",
        "/api/v1/status",
        "/api/v1/output/power",
        "/api/v1/settings/brightness",
        "/api/v1/audio/devices",
        "/api/v1/effects/pause",
        "/api/v1/effects/resume",
        "/api/v1/effects/active",
        "/api/v1/effects/stop",
        "/api/v1/scenes/active",
        "/api/v1/scenes/deactivate",
        "/api/v1/scenes/{id}/zones",
        "/api/v1/scenes/{id}/unassigned-behavior",
        "/api/v1/library/presets/{id}/apply",
    ] {
        assert!(
            body["paths"][retired].is_null(),
            "{retired} must be absent from the document"
        );
    }
    assert!(body["components"]["schemas"]["SystemResource"].is_object());
    for retired in [
        "SetOutputPowerRequest",
        "OutputPowerResponse",
        "OutputPowerStatus",
        "SetBrightnessRequest",
        "PauseEffectResponse",
        "ResumeEffectResponse",
        "AssignDevicesRequest",
        "ApplyControlChangesRequest",
        "UpdateDisplayFaceControlsRequest",
    ] {
        assert!(
            body["components"]["schemas"][retired].is_null(),
            "{retired} must be absent from the schema components"
        );
    }
    assert!(body["paths"]["/api/v1/effects/{id}/apply"].is_object());
    assert_ne!(
        body["paths"]["/api/v1/effects/{id}/apply"]["post"]["requestBody"]["required"],
        true
    );
    assert!(body["components"]["schemas"]["SpatialLayout"].is_object());
    assert!(body["paths"]["/api/v1/control-surfaces"].is_object());
    assert!(body["components"]["schemas"]["ControlSurfaceDocument"].is_object());
    assert!(body["components"]["schemas"]["PatchControlsRequest"].is_object());
    assert!(body["components"]["schemas"]["ControlFieldDescriptor"].is_object());
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
        ("/api/v1/capture/source", "put"),
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
            "#/components/schemas/ApiErrorBody"
        );
    }
    assert!(body["components"]["schemas"]["CaptureAuthorizationResponse"].is_object());
    assert!(body["components"]["schemas"]["CapturePickerResponse"].is_object());
    assert!(body["components"]["schemas"]["CaptureMonitorList"].is_object());
    assert!(body["components"]["schemas"]["ProtectedSourceGrantOwner"].is_object());
}

fn target_operations() -> BTreeSet<(String, String)> {
    let target: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/rest_v1/spec78-target-manifest.json"))
            .expect("Spec 78 target manifest should parse");
    target["paths"]
        .as_array()
        .expect("target manifest paths should be an array")
        .iter()
        .flat_map(|route| {
            let path = route["path"]
                .as_str()
                .expect("target route should have a path")
                .to_owned();
            route["methods"]
                .as_array()
                .expect("target route methods should be an array")
                .iter()
                .map(move |method| {
                    (
                        method
                            .as_str()
                            .expect("target method should be a string")
                            .to_ascii_lowercase(),
                        path.clone(),
                    )
                })
        })
        .collect()
}

fn documented_operations(document: &serde_json::Value) -> BTreeSet<(String, String)> {
    const METHODS: [&str; 5] = ["delete", "get", "patch", "post", "put"];

    document["paths"]
        .as_object()
        .expect("OpenAPI paths should be an object")
        .iter()
        .flat_map(|(path, item)| {
            METHODS
                .into_iter()
                .filter(move |method| item[*method].is_object())
                .map(move |method| (method.to_owned(), path.clone()))
        })
        .collect()
}

fn assert_local_refs_resolve(root: &serde_json::Value, value: &serde_json::Value) {
    match value {
        serde_json::Value::Object(object) => {
            if let Some(reference) = object.get("$ref").and_then(serde_json::Value::as_str)
                && let Some(pointer) = reference.strip_prefix('#')
            {
                assert!(
                    root.pointer(pointer).is_some(),
                    "unresolved OpenAPI reference {reference}"
                );
            }
            for child in object.values() {
                assert_local_refs_resolve(root, child);
            }
        }
        serde_json::Value::Array(array) => {
            for child in array {
                assert_local_refs_resolve(root, child);
            }
        }
        _ => {}
    }
}

#[test]
fn runtime_document_exactly_matches_the_spec_78_target_manifest() {
    let document = openapi_document();
    let live = documented_operations(&document);
    let target = target_operations();

    assert_eq!(target.len(), 117, "target operation count drifted");
    assert_eq!(live.len(), 117, "live operation count has not converged");
    assert_eq!(
        target
            .iter()
            .map(|(_, path)| path)
            .collect::<BTreeSet<_>>()
            .len(),
        82,
        "target path count drifted"
    );
    assert_eq!(
        live.iter()
            .map(|(_, path)| path)
            .collect::<BTreeSet<_>>()
            .len(),
        82,
        "live path count has not converged"
    );
    assert_eq!(
        live, target,
        "runtime route registration diverged from the locked inventory"
    );
}

#[test]
fn runtime_document_has_complete_operation_contracts() {
    let document = openapi_document();
    let mut operation_ids = BTreeSet::new();
    let paths = document["paths"]
        .as_object()
        .expect("OpenAPI paths should be an object");

    for (method, path) in documented_operations(&document) {
        let operation = &paths[&path][&method];
        let operation_id = operation["operationId"]
            .as_str()
            .expect("every operation should have an operationId");
        assert!(
            operation_ids.insert(operation_id),
            "duplicate {operation_id}"
        );
        assert!(
            operation["summary"]
                .as_str()
                .is_some_and(|value| !value.is_empty())
        );
        assert_eq!(
            operation["tags"].as_array().map(Vec::len),
            Some(1),
            "{method} {path} should have one resource tag"
        );

        let responses = operation["responses"]
            .as_object()
            .expect("every operation should declare responses");
        let successes = responses
            .iter()
            .filter(|(status, _)| status.starts_with('2') || status.as_str() == "101")
            .collect::<Vec<_>>();
        assert!(
            !successes.is_empty(),
            "{method} {path} has no success response"
        );
        if path != "/api/v1/ws" {
            for (status, response) in successes {
                assert!(
                    response["content"]
                        .as_object()
                        .is_some_and(|content| !content.is_empty()),
                    "{method} {path} response {status} has no schema"
                );
                assert!(
                    !response.to_string().contains("#/components/schemas/Value"),
                    "{method} {path} response {status} uses an untyped Value schema"
                );
            }
        }

        for status in [
            "400", "401", "403", "404", "409", "412", "422", "429", "500",
        ] {
            assert_eq!(
                responses[status]["content"]["application/json"]["schema"]["$ref"],
                "#/components/schemas/ApiErrorBody",
                "{method} {path} response {status} must use the shared error contract"
            );
        }

        let expected_parameters = path
            .split('/')
            .filter_map(|segment| segment.strip_prefix('{')?.strip_suffix('}'))
            .collect::<BTreeSet<_>>();
        let parameters = operation["parameters"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default();
        let actual_parameters = parameters
            .iter()
            .filter(|parameter| parameter["in"] == "path")
            .filter_map(|parameter| parameter["name"].as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(actual_parameters, expected_parameters, "{method} {path}");
        for parameter in parameters
            .iter()
            .filter(|parameter| parameter["in"] == "path")
        {
            assert_eq!(parameter["required"], true, "{method} {path}");
            assert_eq!(parameter["schema"]["type"], "string", "{method} {path}");
        }
    }

    assert_eq!(operation_ids.len(), 117);
    let schemas = &document["components"]["schemas"];
    assert!(schemas["Vec"].is_null());
    assert!(schemas["ListResponse"].is_null());
    assert!(schemas["Option"].is_null());
    for schema in [
        "CaptureMonitorList",
        "ConfigKeySchemaEntryList",
        "DeviceSummaryListResponse",
        "DisplaySummaryList",
        "MediaAssetRecordListResponse",
        "SimulatedDisplayConfigList",
    ] {
        assert!(schemas[schema].is_object(), "missing schema {schema}");
    }
    assert_local_refs_resolve(&document, &document);
}

#[test]
fn runtime_document_records_real_statuses_bodies_and_media_types() {
    let document = openapi_document();
    let paths = &document["paths"];

    for (path, method, status) in [
        ("/health", "get", "503"),
        ("/api/v1/assets", "post", "201"),
        ("/api/v1/attachments/templates", "post", "201"),
        ("/api/v1/devices/discover", "post", "202"),
        ("/api/v1/effects/install", "post", "201"),
        ("/api/v1/layouts/{id}", "delete", "202"),
        ("/api/v1/layouts/{id}/apply", "post", "202"),
        ("/api/v1/scene/zones", "post", "201"),
        ("/api/v1/scene/zones/{zone}/layers", "post", "201"),
        ("/api/v1/scenes", "post", "201"),
        ("/api/v1/scenes/snapshot", "post", "201"),
        ("/api/v1/simulators/displays", "post", "201"),
        ("/api/v1/ws", "get", "101"),
    ] {
        assert!(
            paths[path][method]["responses"][status].is_object(),
            "{method} {path} should document {status}"
        );
    }

    for (path, method, schema, required) in [
        ("/api/v1/output", "patch", "OutputPatchRequest", true),
        ("/api/v1/scene", "patch", "ScenePatchRequest", true),
        (
            "/api/v1/scene/zones/{zone}/layers/{layer}/controls",
            "patch",
            "PatchControlsRequest",
            true,
        ),
        (
            "/api/v1/control-surfaces/{id}/values",
            "patch",
            "PatchControlsRequest",
            true,
        ),
        (
            "/api/v1/displays/{id}/face/controls",
            "patch",
            "PatchControlsRequest",
            true,
        ),
        ("/api/v1/devices/discover", "post", "DiscoverRequest", false),
        (
            "/api/v1/effects/{id}/apply",
            "post",
            "ApplyEffectRequest",
            false,
        ),
    ] {
        let body = &paths[path][method]["requestBody"];
        assert_eq!(body["required"], required, "{method} {path}");
        assert_eq!(
            body["content"]["application/json"]["schema"]["$ref"],
            format!("#/components/schemas/{schema}"),
            "{method} {path}"
        );
    }

    let schemas = &document["components"]["schemas"];
    let control_variants = schemas["ControlValue"]["oneOf"]
        .as_array()
        .expect("canonical ControlValue should be a tagged union");
    let control_kinds = control_variants
        .iter()
        .filter_map(|variant| variant["properties"]["kind"]["enum"][0].as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(control_kinds.len(), 19);
    assert!(control_kinds.contains("float"));
    assert!(control_kinds.contains("map"));
    assert!(schemas["EffectControlValue"].is_object());
    assert_eq!(
        schemas["PatchControlsRequest"]["properties"]["values"]["additionalProperties"]["$ref"],
        "#/components/schemas/ControlValue"
    );

    for (path, content_type) in [
        ("/api/v1/assets/{id}/blob", "application/octet-stream"),
        ("/api/v1/assets/{id}/thumbnail", "image/webp"),
        ("/api/v1/displays/{id}/frame", "image/jpeg"),
        ("/api/v1/effects/{id}/cover", "image/jpeg"),
        ("/api/v1/simulators/displays/{id}/frame", "image/jpeg"),
    ] {
        assert_eq!(
            paths[path]["get"]["responses"]["200"]["content"][content_type]["schema"]["format"],
            "binary",
            "GET {path}"
        );
    }

    for (path, method, expected) in [
        ("/api/v1/assets", "post", &["rename_duplicate", "type"][..]),
        (
            "/api/v1/attachments/templates",
            "get",
            &[
                "offset",
                "limit",
                "category",
                "vendor",
                "origin",
                "q",
                "controller_id",
                "model",
                "slot_id",
                "led_min",
                "led_max",
            ][..],
        ),
        ("/api/v1/config/keys/{key}", "put", &["live"][..]),
        ("/api/v1/config/keys/{key}", "delete", &["live"][..]),
        ("/api/v1/config/reset", "post", &["live"][..]),
        (
            "/api/v1/control-surfaces",
            "get",
            &["device_id", "driver_id", "include_driver"][..],
        ),
        (
            "/api/v1/devices",
            "get",
            &[
                "offset",
                "limit",
                "status",
                "backend_id",
                "driver",
                "q",
                "include",
            ][..],
        ),
        ("/api/v1/displays/{id}/face", "delete", &["scope"][..]),
        (
            "/api/v1/effects",
            "get",
            &[
                "category",
                "audio_reactive",
                "screen_reactive",
                "input_reactive",
                "source",
                "q",
                "include",
            ][..],
        ),
        ("/api/v1/layouts", "get", &["offset", "limit", "active"][..]),
    ] {
        let parameters = paths[path][method]["parameters"]
            .as_array()
            .expect("query-bearing operation should declare parameters");
        let actual = parameters
            .iter()
            .filter(|parameter| parameter["in"] == "query")
            .map(|parameter| {
                assert_ne!(parameter["required"], true, "{method} {path}");
                parameter["name"]
                    .as_str()
                    .expect("query parameter should have a name")
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(
            actual,
            expected.iter().copied().collect::<BTreeSet<_>>(),
            "{method} {path}"
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
