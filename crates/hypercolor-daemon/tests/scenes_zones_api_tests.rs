use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::SystemTime;

use axum::body::Body;
use http::{Request, StatusCode};
use hypercolor_core::config::ConfigManager;
use hypercolor_core::effect::EffectEntry;
use hypercolor_daemon::api::{self, AppState};
use hypercolor_daemon::layout_auto_exclusions::LayoutAutoExclusionKey;
use hypercolor_types::effect::{
    ControlValue, EffectCategory, EffectId, EffectMetadata, EffectSource, EffectState,
};
use hypercolor_types::scene::{SceneId, ZoneId};
use hypercolor_types::spatial::{
    EdgeBehavior, LedTopology, NormalizedPosition, Output, SamplingMode, SpatialLayout,
    StripDirection,
};
use tower::ServiceExt;
use uuid::Uuid;

static DATA_DIR_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

fn isolated_state_with_tempdir() -> (Arc<AppState>, tempfile::TempDir) {
    let _lock = DATA_DIR_LOCK
        .lock()
        .expect("data dir lock should not be poisoned");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let data_dir = tempdir.path().join("data");
    std::fs::create_dir_all(&data_dir).expect("temp data dir should be created");
    ConfigManager::set_data_dir_override(Some(data_dir));
    let state = AppState::new();
    ConfigManager::set_data_dir_override(None);
    (Arc::new(state), tempdir)
}

fn test_app_with_state(state: Arc<AppState>) -> axum::Router {
    api::build_router(state, None)
}

async fn send(app: &axum::Router, request: Request<Body>) -> axum::response::Response {
    app.clone()
        .oneshot(request)
        .await
        .expect("request should succeed")
}

async fn body_json(response: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body should read");
    serde_json::from_slice(&bytes).expect("response body should be JSON")
}

fn json_request(method: &str, uri: String, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .expect("request should build")
}

fn empty_request(method: &str, uri: String) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .body(Body::empty())
        .expect("request should build")
}

fn if_match(mut request: Request<Body>, version: u64) -> Request<Body> {
    request.headers_mut().insert(
        http::header::IF_MATCH,
        http::HeaderValue::from_str(&format!("\"{version}\"")).expect("valid etag"),
    );
    request
}

fn response_etag(response: &axum::response::Response) -> String {
    response
        .headers()
        .get(http::header::ETAG)
        .and_then(|value| value.to_str().ok())
        .expect("response should include ETag")
        .to_owned()
}

fn sample_effect(name: &str) -> EffectMetadata {
    EffectMetadata {
        id: EffectId::new(Uuid::now_v7()),
        name: name.to_owned(),
        author: "test".to_owned(),
        version: "0.1.0".to_owned(),
        description: format!("{name} description"),
        category: EffectCategory::Ambient,
        tags: vec!["test".to_owned()],
        controls: Vec::new(),
        presets: Vec::new(),
        audio_reactive: false,
        screen_reactive: false,
        source: EffectSource::Native {
            path: format!("builtin/{name}").into(),
        },
        license: None,
    }
}

fn sample_zone(id: &str) -> Output {
    Output {
        id: id.to_owned(),
        name: id.to_owned(),
        device_id: format!("mock:{id}"),
        zone_name: None,
        position: NormalizedPosition::new(0.25, 0.25),
        size: NormalizedPosition::new(0.5, 0.5),
        rotation: 0.0,
        scale: 1.0,
        display_order: 0,
        orientation: None,
        topology: LedTopology::Strip {
            count: 1,
            direction: StripDirection::LeftToRight,
        },
        led_positions: Vec::new(),
        led_mapping: None,
        sampling_mode: Some(SamplingMode::Bilinear),
        edge_behavior: Some(EdgeBehavior::Clamp),
        shape: None,
        shape_preset: None,
        attachment: None,
        brightness: None,
    }
}

fn sample_layout(zone_id: &str) -> SpatialLayout {
    SpatialLayout {
        id: format!("layout-{zone_id}"),
        name: format!("Layout {zone_id}"),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones: vec![sample_zone(zone_id)],
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    }
}

async fn seed_primary_group(state: &Arc<AppState>, zone_id: &str) {
    let metadata = sample_effect("Primary");
    {
        let mut registry = state.effect_registry.write().await;
        registry.register(EffectEntry {
            metadata: metadata.clone(),
            source_path: "/tmp/primary.rs".into(),
            modified: SystemTime::now(),
            state: EffectState::Loading,
        });
    }
    let mut manager = state.scene_manager.write().await;
    manager
        .upsert_primary_group(
            &metadata,
            HashMap::<String, ControlValue>::new(),
            None,
            sample_layout(zone_id),
        )
        .expect("primary group should seed");
}

#[tokio::test]
async fn zone_crud_uses_groups_revision_etags() {
    let (state, _tmp) = isolated_state_with_tempdir();
    let app = test_app_with_state(Arc::clone(&state));

    let response = send(
        &app,
        empty_request("GET", "/api/v1/scenes/default/zones".into()),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_etag(&response), "\"0\"");
    let json = body_json(response).await;
    assert_eq!(
        json["data"]["items"].as_array().expect("items array").len(),
        1
    );
    assert_eq!(json["data"]["items"][0]["role"], "primary");

    let response = send(
        &app,
        if_match(
            json_request(
                "POST",
                "/api/v1/scenes/default/zones".into(),
                serde_json::json!({
                    "name": "Desk",
                    "color": "#80ffea"
                }),
            ),
            0,
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(response_etag(&response), "\"1\"");
    let json = body_json(response).await;
    let zone_id = json["data"]["zone"]["id"]
        .as_str()
        .expect("zone id should be a string")
        .to_owned();
    assert_eq!(json["data"]["zone"]["role"], "custom");
    assert_eq!(json["data"]["groups_revision"], 1);

    let response = send(
        &app,
        json_request(
            "PATCH",
            format!("/api/v1/scenes/default/zones/{zone_id}"),
            serde_json::json!({
                "name": "Desk Glow",
                "brightness": 1.7
            }),
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_etag(&response), "\"1\"");
    let json = body_json(response).await;
    assert_eq!(json["data"]["zone"]["name"], "Desk Glow");
    assert_eq!(json["data"]["zone"]["brightness"], 1.0);

    let response = send(
        &app,
        if_match(
            empty_request("DELETE", format!("/api/v1/scenes/default/zones/{zone_id}")),
            0,
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::PRECONDITION_FAILED);
    assert_eq!(response_etag(&response), "\"1\"");

    let response = send(
        &app,
        if_match(
            empty_request("DELETE", format!("/api/v1/scenes/default/zones/{zone_id}")),
            1,
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_etag(&response), "\"2\"");
}

#[tokio::test]
async fn device_assignment_moves_existing_zone_to_target_zone() {
    let (state, _tmp) = isolated_state_with_tempdir();
    seed_primary_group(&state, "primary-zone").await;
    let app = test_app_with_state(Arc::clone(&state));

    let response = send(
        &app,
        json_request(
            "POST",
            "/api/v1/scenes/default/zones".into(),
            serde_json::json!({ "name": "Room" }),
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let revision = response_etag(&response);
    let json = body_json(response).await;
    let zone_id = json["data"]["zone"]["id"]
        .as_str()
        .expect("zone id should be a string")
        .to_owned();
    let revision = revision
        .trim_matches('"')
        .parse::<u64>()
        .expect("etag should be revision");

    let response = send(
        &app,
        if_match(
            json_request(
                "POST",
                format!("/api/v1/scenes/default/zones/{zone_id}/devices"),
                serde_json::json!({
                    "device_zones": [{ "id": "primary-zone" }]
                }),
            ),
            revision,
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    let groups = json["data"]["items"].as_array().expect("groups array");
    let primary = groups
        .iter()
        .find(|group| group["role"] == "primary")
        .expect("primary group should exist");
    let primary_group_id = primary["id"].as_str().expect("primary id should be string");
    let custom = groups
        .iter()
        .find(|group| group["id"] == zone_id)
        .expect("custom group should exist");
    assert_eq!(
        primary["layout"]["zones"].as_array().expect("zones").len(),
        0
    );
    assert_eq!(
        custom["layout"]["zones"][0]["id"]
            .as_str()
            .expect("zone id should be string"),
        "primary-zone"
    );
    let primary_key = LayoutAutoExclusionKey::zone(
        SceneId::DEFAULT,
        ZoneId(Uuid::parse_str(primary_group_id).expect("primary id should be uuid")),
    );
    {
        let exclusions = state.layout_auto_exclusions.read().await;
        assert_eq!(
            exclusions.get(&primary_key),
            Some(&HashSet::from(["mock:primary-zone".to_owned()]))
        );
    }

    let next_revision = json["data"]["groups_revision"]
        .as_u64()
        .expect("revision should be u64");
    let response = send(
        &app,
        if_match(
            empty_request(
                "DELETE",
                format!("/api/v1/scenes/default/zones/{zone_id}/devices/primary-zone"),
            ),
            next_revision,
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    let groups = json["data"]["items"].as_array().expect("groups array");
    assert!(groups.iter().all(|group| {
        group["layout"]["zones"]
            .as_array()
            .expect("zones array")
            .is_empty()
    }));
    let custom_key = LayoutAutoExclusionKey::zone(
        SceneId::DEFAULT,
        ZoneId(Uuid::parse_str(&zone_id).expect("custom id should be uuid")),
    );
    {
        let exclusions = state.layout_auto_exclusions.read().await;
        assert_eq!(
            exclusions.get(&custom_key),
            Some(&HashSet::from(["mock:primary-zone".to_owned()]))
        );
    }

    let revision = json["data"]["groups_revision"]
        .as_u64()
        .expect("revision should be u64");
    let mut invalid_zone = sample_zone("primary-zone");
    invalid_zone.sampling_mode = Some(SamplingMode::AreaAverage {
        radius_x: -1.0,
        radius_y: 0.0,
    });
    let response = send(
        &app,
        if_match(
            json_request(
                "POST",
                format!("/api/v1/scenes/default/zones/{zone_id}/devices"),
                serde_json::json!({ "device_zones": [invalid_zone] }),
            ),
            revision,
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let response = send(
        &app,
        if_match(
            json_request(
                "POST",
                format!("/api/v1/scenes/default/zones/{zone_id}/devices"),
                serde_json::json!({
                    "device_zones": [sample_zone("primary-zone")]
                }),
            ),
            revision,
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    {
        let exclusions = state.layout_auto_exclusions.read().await;
        assert!(
            !exclusions.contains_key(&custom_key),
            "re-added devices should clear the target zone exclusion"
        );
    }
}

#[tokio::test]
async fn unassigned_behavior_route_validates_fallback_zone() {
    let (state, _tmp) = isolated_state_with_tempdir();
    let app = test_app_with_state(Arc::clone(&state));
    let response = send(
        &app,
        json_request(
            "POST",
            "/api/v1/scenes/default/zones".into(),
            serde_json::json!({ "name": "Fallback" }),
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let json = body_json(response).await;
    let zone_id = json["data"]["zone"]["id"]
        .as_str()
        .expect("zone id should be a string")
        .to_owned();
    let revision = json["data"]["groups_revision"]
        .as_u64()
        .expect("revision should be u64");

    let response = send(
        &app,
        if_match(
            json_request(
                "PATCH",
                "/api/v1/scenes/default/unassigned-behavior".into(),
                serde_json::json!({
                    "unassigned_behavior": { "fallback": zone_id }
                }),
            ),
            revision,
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(
        json["data"]["unassigned_behavior"]["fallback"]
            .as_str()
            .expect("fallback id should be a string"),
        zone_id
    );

    let response = send(
        &app,
        json_request(
            "PATCH",
            "/api/v1/scenes/default/unassigned-behavior".into(),
            serde_json::json!({
                "unassigned_behavior": {
                    "fallback": Uuid::now_v7().to_string()
                }
            }),
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn status_advertises_multi_zone_backend_capabilities() {
    let (state, _tmp) = isolated_state_with_tempdir();
    let app = test_app_with_state(Arc::clone(&state));

    let response = send(&app, empty_request("GET", "/api/v1/status".into())).await;

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    let capabilities = json["data"]["capabilities"]
        .as_array()
        .expect("capabilities should be an array")
        .iter()
        .map(|value| value.as_str().expect("capability should be string"))
        .collect::<Vec<_>>();
    assert!(capabilities.contains(&"multi-zone-sampling"));
    assert!(capabilities.contains(&"zone-crud"));
    assert!(capabilities.contains(&"zone-device-assignment"));
    assert!(capabilities.contains(&"zone-layout-edit"));
    assert!(capabilities.contains(&"zone-preview-frames"));
    assert!(capabilities.contains(&"scene-unassigned-behavior-write"));
}

#[tokio::test]
async fn zone_layout_route_merges_placement_and_rejects_output_changes() {
    let (state, _tmp) = isolated_state_with_tempdir();
    seed_primary_group(&state, "primary-zone").await;
    let app = test_app_with_state(Arc::clone(&state));

    // Resolve the primary zone id and the current groups_revision.
    let response = send(
        &app,
        empty_request("GET", "/api/v1/scenes/default/zones".into()),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    let revision = json["data"]["groups_revision"]
        .as_u64()
        .expect("revision should be u64");
    let zone_id = json["data"]["items"]
        .as_array()
        .expect("groups array")
        .iter()
        .find(|group| group["role"] == "primary")
        .and_then(|group| group["id"].as_str())
        .expect("primary zone id")
        .to_owned();

    // A placement merge: same output, new placement, attempted hardware
    // rewrite. Placement applies; the device binding is preserved.
    let mut layout = sample_layout("primary-zone");
    layout.zones[0].display_order = 9;
    layout.zones[0].device_id = "mock:HIJACKED".to_owned();
    let response = send(
        &app,
        if_match(
            json_request(
                "PUT",
                format!("/api/v1/scenes/default/zones/{zone_id}/layout"),
                serde_json::to_value(&layout).expect("layout should serialize"),
            ),
            revision,
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    let output = &json["data"]["zone"]["layout"]["zones"][0];
    assert_eq!(output["display_order"].as_i64(), Some(9));
    assert_eq!(output["device_id"].as_str(), Some("mock:primary-zone"));
    let next_revision = json["data"]["groups_revision"]
        .as_u64()
        .expect("revision should be u64");

    let mut invalid = sample_layout("primary-zone");
    invalid.zones[0].sampling_mode = Some(SamplingMode::AreaAverage {
        radius_x: 0.0,
        radius_y: -1.0,
    });
    let response = send(
        &app,
        if_match(
            json_request(
                "PUT",
                format!("/api/v1/scenes/default/zones/{zone_id}/layout"),
                serde_json::to_value(&invalid).expect("layout should serialize"),
            ),
            next_revision,
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // An added output is rejected with 422 — adds route through the
    // device endpoints, not the layout endpoint.
    let mut bad = sample_layout("primary-zone");
    bad.zones.push(sample_zone("ghost-zone"));
    let response = send(
        &app,
        if_match(
            json_request(
                "PUT",
                format!("/api/v1/scenes/default/zones/{zone_id}/layout"),
                serde_json::to_value(&bad).expect("layout should serialize"),
            ),
            next_revision,
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn created_scenes_are_born_with_a_default_zone() {
    let (state, _tmp) = isolated_state_with_tempdir();
    let app = test_app_with_state(Arc::clone(&state));

    let response = send(
        &app,
        json_request(
            "POST",
            "/api/v1/scenes".into(),
            serde_json::json!({ "name": "Studio Scene" }),
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let scene_id = body_json(response).await["data"]["id"]
        .as_str()
        .expect("scene id should be a string")
        .to_owned();

    // A fresh scene has a selectable Default zone, not an empty group set.
    let response = send(
        &app,
        empty_request("GET", format!("/api/v1/scenes/{scene_id}/zones")),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    let zones = json["data"]["items"].as_array().expect("zones array");
    assert_eq!(zones.len(), 1);
    assert_eq!(zones[0]["role"], "primary");
}
