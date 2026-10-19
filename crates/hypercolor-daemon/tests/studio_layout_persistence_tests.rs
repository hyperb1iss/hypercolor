//! A Studio placement saved on a primary zone that follows the active
//! layout lands in that layout, so every path that rebuilds the zone from
//! the active layout (effect and preset applies, layout publications, the
//! startup restore) keeps it.
#![cfg(feature = "persistence-test-hooks")]

use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use axum::body::Body;
use http::{Request, StatusCode, header};
use hypercolor_core::effect::EffectEntry;
use hypercolor_daemon::api;
use hypercolor_daemon::app_state::{AppState, AppStateBuilder};
use hypercolor_daemon::{layout_store, runtime_state};
use hypercolor_types::control::ControlValue;
use hypercolor_types::effect::{
    ControlDefinition, ControlKind, ControlType, EffectCategory, EffectId, EffectMetadata,
    EffectSource, EffectState, PresetTemplate,
};
use hypercolor_types::library::PresetId;
use hypercolor_types::scene::{SceneId, ZoneRole};
use hypercolor_types::spatial::{
    EdgeBehavior, LedTopology, NormalizedPosition, Output, SamplingMode, SpatialLayout,
    StripDirection,
};
use serde_json::json;
use tower::ServiceExt;
use uuid::Uuid;

const ACTIVE_LAYOUT_ID: &str = "layout-active";
const AUTO_POSITION: (f64, f64) = (0.25, 0.25);
const SAVED_POSITION: (f64, f64) = (0.8, 0.7);

struct Rig {
    state: Arc<AppState>,
    app: axum::Router,
    effect_id: EffectId,
    tmp: tempfile::TempDir,
}

impl Rig {
    fn layouts_path(&self) -> PathBuf {
        self.tmp.path().join("data").join("layouts.json")
    }
}

/// Boot the shape a fresh user rig has: one stored layout that is also the
/// published one, and an effect running in the Default scene's primary zone.
async fn rig_with_running_effect() -> Rig {
    let tmp = tempfile::tempdir().expect("tempdir should be created");
    let data_dir = tmp.path().join("data");
    std::fs::create_dir_all(&data_dir).expect("temp data dir should be created");
    let layouts_path = data_dir.join("layouts.json");
    let state = Arc::new(
        AppStateBuilder::new(data_dir)
            .with_runtime_state_path(tmp.path().join("runtime-state.json"))
            .build(),
    );
    let app = api::build_router(Arc::clone(&state), None);
    install_active_layout(&state, &layouts_path, active_layout()).await;
    let aurora = effect("Aurora");
    register(&state, &aurora).await;
    let (started, _) = send(
        &state,
        &app,
        json_request(
            "POST",
            format!("/api/v1/effects/{}/apply", aurora.id),
            json!({}),
        ),
    )
    .await;
    assert_eq!(started.status(), StatusCode::OK);
    Rig {
        state,
        app,
        effect_id: aurora.id,
        tmp,
    }
}

/// Store `layout`, persist the store the way a running daemon keeps it,
/// and publish it as the active layout.
async fn install_active_layout(state: &AppState, layouts_path: &Path, layout: SpatialLayout) {
    let fixture = state.domains.layout.test_fixture();
    let stored = {
        let mut catalog = fixture.catalog().write().await;
        catalog.insert(layout.id.clone(), layout.clone());
        let mut stored = catalog.values().cloned().collect::<Vec<_>>();
        stored.sort_by(|left, right| left.id.cmp(&right.id));
        stored
    };
    std::fs::write(
        layouts_path,
        serde_json::to_vec_pretty(&stored).expect("layout store should serialize"),
    )
    .expect("layout store should be written");
    fixture.replace_current(layout);
}

/// Run `work` to completion, retiring every layout publication it queues
/// the way the render thread would. Returns how many layouts it published.
async fn drive<F: Future>(state: &AppState, work: F) -> (F::Output, usize) {
    tokio::pin!(work);
    let executor = state.layout_publication_test_executor();
    let mut publications = Vec::new();
    let output = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            tokio::select! {
                output = &mut work => break output,
                () = tokio::time::sleep(Duration::from_millis(1)) => {
                    if executor.pending_layout_publications() > 0 {
                        let executor = executor.clone();
                        publications.push(tokio::spawn(async move {
                            executor.execute_next_layout_publication().await
                        }));
                    }
                }
            }
        }
    })
    .await
    .expect("work should finish");
    let mut published = 0;
    for publication in publications {
        let layout = publication
            .await
            .expect("layout publication task should not panic")
            .expect("layout publication should succeed");
        published += usize::from(layout.is_some());
    }
    (output, published)
}

async fn send(
    state: &AppState,
    app: &axum::Router,
    request: Request<Body>,
) -> (axum::response::Response, usize) {
    let (response, published) = drive(state, app.clone().oneshot(request)).await;
    (response.expect("request should succeed"), published)
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

fn etag_revision(response: &axum::response::Response) -> u64 {
    response
        .headers()
        .get(header::ETAG)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim_matches('"').parse().ok())
        .expect("mutation responses carry the scene revision")
}

fn effect(name: &str) -> EffectMetadata {
    EffectMetadata {
        id: EffectId::new(Uuid::now_v7()),
        name: name.to_owned(),
        author: "test".to_owned(),
        version: "0.1.0".to_owned(),
        description: format!("{name} description"),
        category: EffectCategory::Ambient,
        tags: vec!["test".to_owned()],
        controls: vec![ControlDefinition {
            id: "speed".to_owned(),
            name: "Speed".to_owned(),
            kind: ControlKind::Number,
            control_type: ControlType::Slider,
            default_value: ControlValue::Float(0.25),
            min: Some(0.0),
            max: Some(1.0),
            step: Some(0.05),
            labels: Vec::new(),
            group: None,
            tooltip: None,
            aspect_lock: None,
            preview_source: None,
            binding: None,
        }],
        presets: vec![PresetTemplate {
            id: PresetId::stable("test-fast"),
            name: "Fast".to_owned(),
            description: None,
            controls: HashMap::from([("speed".to_owned(), ControlValue::Float(0.9))]),
        }],
        audio_reactive: false,
        screen_reactive: false,
        input_reactive: false,
        source: EffectSource::Native {
            path: format!("builtin/{name}").into(),
        },
        license: None,
    }
}

async fn register(state: &Arc<AppState>, metadata: &EffectMetadata) {
    let _ = state
        .domains
        .effects
        .register(EffectEntry {
            metadata: metadata.clone(),
            source_path: format!("/tmp/{}.rs", metadata.name).into(),
            modified: SystemTime::now(),
            state: EffectState::Loading,
        })
        .await;
}

fn strip(id: &str, segment: &str) -> Output {
    Output {
        id: id.to_owned(),
        name: id.to_owned(),
        device_id: "mock:controller".to_owned(),
        zone_name: Some(segment.to_owned()),
        position: NormalizedPosition::new(0.25, 0.25),
        size: NormalizedPosition::new(0.5, 0.1),
        rotation: 0.0,
        scale: 1.0,
        display_order: 0,
        orientation: None,
        topology: LedTopology::Strip {
            count: 4,
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

/// Two strips, both at the auto-layout default position.
fn active_layout() -> SpatialLayout {
    SpatialLayout {
        id: ACTIVE_LAYOUT_ID.to_owned(),
        name: "Active".to_owned(),
        description: Some("The rig".to_owned()),
        canvas_width: 640,
        canvas_height: 480,
        zones: vec![strip("strip-a", "ch1"), strip("strip-b", "ch2")],
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        version: 3,
    }
}

async fn primary_zone(app: &axum::Router) -> serde_json::Value {
    let response = app
        .clone()
        .oneshot(empty_request("GET", "/api/v1/scene".into()))
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::OK);
    let document = body_json(response).await;
    document["data"]["zones"]
        .as_array()
        .expect("zones array")
        .iter()
        .find(|zone| zone["role"] == "primary")
        .cloned()
        .expect("the live tree always carries a primary zone")
}

async fn scene_revision(app: &axum::Router) -> u64 {
    let response = app
        .clone()
        .oneshot(empty_request("GET", "/api/v1/scene".into()))
        .await
        .expect("request should succeed");
    etag_revision(&response)
}

// Positions are f32 on the wire and in the store; round so 0.8f32 compares
// as 0.8.
fn rounded(x: f64, y: f64) -> (f64, f64) {
    let round = |value: f64| (value * 1000.0).round() / 1000.0;
    (round(x), round(y))
}

fn strip_a_in_zone(zone: &serde_json::Value) -> (f64, f64) {
    let member_id = zone["members"]
        .as_array()
        .expect("members")
        .iter()
        .find(|member| member["segment"] == "ch1")
        .expect("strip-a member")["id"]
        .clone();
    let placement = zone["layout"]["placements"]
        .as_array()
        .expect("placements")
        .iter()
        .find(|placement| placement["member"] == member_id)
        .expect("strip-a placement");
    rounded(
        placement["position"]["x"].as_f64().expect("x"),
        placement["position"]["y"].as_f64().expect("y"),
    )
}

fn strip_a_in_layout(layout: &SpatialLayout) -> (f64, f64) {
    let output = layout
        .zones
        .iter()
        .find(|output| output.id == "strip-a")
        .expect("strip-a output");
    rounded(f64::from(output.position.x), f64::from(output.position.y))
}

/// Studio's Save: move strip-a and persist through the per-zone route,
/// guarded by the revision the editor last saw. Returns the response and
/// how many layouts the Save published.
async fn studio_moves_strip_a(rig: &Rig) -> (axum::response::Response, usize) {
    let zone = primary_zone(&rig.app).await;
    let zone_id = zone["id"].as_str().expect("zone id").to_owned();
    let strip_a_member = zone["members"]
        .as_array()
        .expect("members")
        .iter()
        .find(|member| member["segment"] == "ch1")
        .expect("strip-a member")["id"]
        .clone();
    let mut placements = zone["layout"]["placements"].clone();
    for placement in placements.as_array_mut().expect("placements") {
        if placement["member"] == strip_a_member {
            placement["position"] = json!({ "x": SAVED_POSITION.0, "y": SAVED_POSITION.1 });
        }
    }
    let mut request = json_request(
        "PUT",
        format!("/api/v1/scene/zones/{zone_id}/layout"),
        json!({ "placements": placements }),
    );
    let revision = scene_revision(&rig.app).await;
    request.headers_mut().insert(
        header::IF_MATCH,
        format!("\"{revision}\"").parse().expect("valid If-Match"),
    );
    let (saved, published) = send(&rig.state, &rig.app, request).await;
    assert_eq!(saved.status(), StatusCode::OK, "Studio Save should land");
    assert_eq!(
        strip_a_in_zone(&primary_zone(&rig.app).await),
        SAVED_POSITION,
        "the saved position is live right after Save"
    );
    (saved, published)
}

async fn apply_preset(rig: &Rig) {
    let preset_id = PresetId::stable("test-fast");
    let (applied, _) = send(
        &rig.state,
        &rig.app,
        json_request(
            "POST",
            format!(
                "/api/v1/effects/{}/presets/{preset_id}/apply",
                rig.effect_id
            ),
            json!({}),
        ),
    )
    .await;
    assert_eq!(applied.status(), StatusCode::OK);
}

#[tokio::test]
async fn saved_primary_zone_placement_survives_a_preset_apply() {
    let rig = rig_with_running_effect().await;
    studio_moves_strip_a(&rig).await;

    apply_preset(&rig).await;

    assert_eq!(
        strip_a_in_zone(&primary_zone(&rig.app).await),
        SAVED_POSITION,
        "changing presets must not move a strip the user positioned and saved in Studio"
    );
}

#[tokio::test]
async fn saved_primary_zone_placement_survives_an_effect_switch() {
    let rig = rig_with_running_effect().await;
    studio_moves_strip_a(&rig).await;

    let nebula = effect("Nebula");
    register(&rig.state, &nebula).await;
    let (applied, _) = send(
        &rig.state,
        &rig.app,
        json_request(
            "POST",
            format!("/api/v1/effects/{}/apply", nebula.id),
            json!({}),
        ),
    )
    .await;
    assert_eq!(applied.status(), StatusCode::OK);

    assert_eq!(
        strip_a_in_zone(&primary_zone(&rig.app).await),
        SAVED_POSITION,
        "switching effects must not move a strip the user positioned and saved in Studio"
    );
}

#[tokio::test]
async fn saved_primary_zone_placement_lands_in_the_active_layout_and_its_store() {
    let rig = rig_with_running_effect().await;
    let (saved, published) = studio_moves_strip_a(&rig).await;
    assert_eq!(published, 1, "the Save republishes the active layout");

    assert_eq!(
        etag_revision(&saved),
        scene_revision(&rig.app).await,
        "the Save answers with the revision the next guarded write needs"
    );

    let published = rig.state.domains.layout.current();
    assert_eq!(strip_a_in_layout(&published), SAVED_POSITION);
    let expected_identity = active_layout();
    assert_eq!(published.id, expected_identity.id);
    assert_eq!(published.name, expected_identity.name);
    assert_eq!(published.description, expected_identity.description);
    assert_eq!(
        (published.canvas_width, published.canvas_height),
        (
            expected_identity.canvas_width,
            expected_identity.canvas_height
        )
    );
    assert_eq!(published.version, expected_identity.version);
    assert_eq!(
        published
            .zones
            .iter()
            .map(|output| output.id.as_str())
            .collect::<Vec<_>>(),
        vec!["strip-a", "strip-b"]
    );

    let (active, _) = send(
        &rig.state,
        &rig.app,
        empty_request("GET", format!("/api/v1/layouts/{ACTIVE_LAYOUT_ID}")),
    )
    .await;
    assert_eq!(active.status(), StatusCode::OK);
    let mut active = body_json(active).await;
    let active: SpatialLayout =
        serde_json::from_value(active["data"].take()).expect("stored layout");
    assert_eq!(strip_a_in_layout(&active), SAVED_POSITION);

    let on_disk = layout_store::load(&rig.layouts_path()).expect("layout store should load");
    assert_eq!(
        strip_a_in_layout(
            on_disk
                .get(ACTIVE_LAYOUT_ID)
                .expect("active layout persisted")
        ),
        SAVED_POSITION
    );
}

#[tokio::test]
async fn saved_primary_zone_placement_survives_reapplying_the_active_layout() {
    let rig = rig_with_running_effect().await;
    studio_moves_strip_a(&rig).await;

    let (applied, published) = send(
        &rig.state,
        &rig.app,
        empty_request("POST", format!("/api/v1/layouts/{ACTIVE_LAYOUT_ID}/apply")),
    )
    .await;
    assert_eq!(applied.status(), StatusCode::OK);
    assert_eq!(published, 1, "the stored layout republishes");

    assert_eq!(
        strip_a_in_zone(&primary_zone(&rig.app).await),
        SAVED_POSITION
    );
    apply_preset(&rig).await;
    assert_eq!(
        strip_a_in_zone(&primary_zone(&rig.app).await),
        SAVED_POSITION
    );
}

#[tokio::test]
async fn saved_primary_zone_placement_survives_a_republication_of_the_live_layout() {
    let rig = rig_with_running_effect().await;
    studio_moves_strip_a(&rig).await;

    // Device hotplug and auto-layout repair republish the live layout.
    let live = rig.state.domains.layout.current();
    let workflows = rig.state.domains.layout.test_workflows();
    let (result, published) = drive(&rig.state, workflows.publish(live)).await;
    result.expect("republication should land");
    assert_eq!(published, 1);

    assert_eq!(
        strip_a_in_zone(&primary_zone(&rig.app).await),
        SAVED_POSITION
    );
}

#[tokio::test]
async fn saved_primary_zone_placement_is_restored_after_a_restart() {
    let rig = rig_with_running_effect().await;
    studio_moves_strip_a(&rig).await;

    // Startup restores the stored layout the runtime session names, then
    // re-aligns the primary zone to it.
    let snapshot = runtime_state::load(&rig.state.runtime_state_path)
        .expect("runtime state should load")
        .expect("runtime state should exist after Save");
    assert_eq!(snapshot.active_scene_id, Some(SceneId::DEFAULT.to_string()));
    assert_eq!(snapshot.active_layout_id.as_deref(), Some(ACTIVE_LAYOUT_ID));
    let persisted_primary = snapshot
        .default_scene_zones
        .iter()
        .find(|zone| zone.role == ZoneRole::Primary)
        .expect("persisted primary zone");
    assert_eq!(strip_a_in_layout(&persisted_primary.layout), SAVED_POSITION);
    let stored = layout_store::load(&rig.layouts_path())
        .expect("layout store should load")
        .remove(ACTIVE_LAYOUT_ID)
        .expect("the runtime session's active layout is stored");

    let restarted = rig_with_running_effect().await;
    install_active_layout(&restarted.state, &restarted.layouts_path(), stored).await;
    apply_preset(&restarted).await;

    assert_eq!(
        strip_a_in_zone(&primary_zone(&restarted.app).await),
        SAVED_POSITION,
        "the first preset change after a restart keeps the saved placement"
    );
}

#[tokio::test]
async fn custom_led_zone_keeps_the_primary_layout_scene_local() {
    let rig = rig_with_running_effect().await;
    let (created, _) = send(
        &rig.state,
        &rig.app,
        json_request(
            "POST",
            "/api/v1/scene/zones".into(),
            json!({ "name": "Desk" }),
        ),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);

    let (_, published) = studio_moves_strip_a(&rig).await;
    assert_eq!(published, 0, "a scene-local Save publishes no layout");

    apply_preset(&rig).await;
    assert_eq!(
        strip_a_in_zone(&primary_zone(&rig.app).await),
        SAVED_POSITION,
        "a scene-local primary layout survives an apply on its own"
    );
    assert_eq!(
        strip_a_in_layout(&rig.state.domains.layout.current()),
        AUTO_POSITION,
        "the active layout is untouched"
    );
    let stored = rig
        .state
        .domains
        .layout
        .resolve(ACTIVE_LAYOUT_ID)
        .await
        .expect("active layout stays stored");
    assert_eq!(strip_a_in_layout(&stored), AUTO_POSITION);
}
