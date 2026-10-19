//! A Studio placement saved on a primary zone that follows the active
//! layout lands in that layout, so every path that rebuilds the zone from
//! the active layout (effect and preset applies, layout publications, scene
//! switches, the startup restore) keeps it. Only what the Save changed
//! lands there: outputs, stored edits, and positions saved from other
//! scenes stay as they were.
#![cfg(feature = "persistence-test-hooks")]

use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use axum::body::Body;
use http::{Request, StatusCode, header};
use hypercolor_core::effect::EffectEntry;
use hypercolor_daemon::LayoutTransactionRejection;
use hypercolor_daemon::api;
use hypercolor_daemon::app_state::{AppState, AppStateBuilder};
use hypercolor_daemon::{layout_store, runtime_state};
use hypercolor_types::control::ControlValue;
use hypercolor_types::effect::{
    ControlDefinition, ControlKind, ControlType, EffectCategory, EffectId, EffectMetadata,
    EffectSource, EffectState, PresetTemplate,
};
use hypercolor_types::event::ChangeTrigger;
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

fn output_in_layout(layout: &SpatialLayout, id: &str) -> (f64, f64) {
    let output = layout
        .zones
        .iter()
        .find(|output| output.id == id)
        .unwrap_or_else(|| panic!("{id} output"));
    rounded(f64::from(output.position.x), f64::from(output.position.y))
}

fn strip_a_in_layout(layout: &SpatialLayout) -> (f64, f64) {
    output_in_layout(layout, "strip-a")
}

fn output_ids(layout: &SpatialLayout) -> Vec<&str> {
    layout
        .zones
        .iter()
        .map(|output| output.id.as_str())
        .collect()
}

/// Studio's Save request: move one segment's member and persist through
/// the per-zone route, guarded by the revision the editor last saw.
async fn studio_save_request(rig: &Rig, segment: &str, to: (f64, f64)) -> Request<Body> {
    let zone = primary_zone(&rig.app).await;
    let zone_id = zone["id"].as_str().expect("zone id").to_owned();
    let member = zone["members"]
        .as_array()
        .expect("members")
        .iter()
        .find(|member| member["segment"] == segment)
        .expect("moved member")["id"]
        .clone();
    let mut placements = zone["layout"]["placements"].clone();
    for placement in placements.as_array_mut().expect("placements") {
        if placement["member"] == member {
            placement["position"] = json!({ "x": to.0, "y": to.1 });
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
    request
}

/// Save a move of one segment's member. Returns the response and how many
/// layouts the Save published.
async fn studio_moves(
    rig: &Rig,
    segment: &str,
    to: (f64, f64),
) -> (axum::response::Response, usize) {
    let request = studio_save_request(rig, segment, to).await;
    let (saved, published) = send(&rig.state, &rig.app, request).await;
    assert_eq!(saved.status(), StatusCode::OK, "Studio Save should land");
    (saved, published)
}

/// Studio's Save of strip-a at [`SAVED_POSITION`].
async fn studio_moves_strip_a(rig: &Rig) -> (axum::response::Response, usize) {
    let (saved, published) = studio_moves(rig, "ch1", SAVED_POSITION).await;
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

async fn create_named_scene(rig: &Rig, name: &str) -> String {
    let (created, _) = send(
        &rig.state,
        &rig.app,
        json_request("POST", "/api/v1/scenes".into(), json!({ "name": name })),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    body_json(created).await["data"]["id"]
        .as_str()
        .expect("scene id")
        .to_owned()
}

async fn activate(rig: &Rig, scene_id: &str) {
    let (activated, _) = send(
        &rig.state,
        &rig.app,
        json_request(
            "POST",
            format!("/api/v1/scenes/{scene_id}/activate"),
            json!({}),
        ),
    )
    .await;
    assert_eq!(activated.status(), StatusCode::OK);
}

/// Store an edit of the active layout without applying it.
async fn store_layout_edit(rig: &Rig, body: serde_json::Value) {
    let (updated, _) = send(
        &rig.state,
        &rig.app,
        json_request("PUT", format!("/api/v1/layouts/{ACTIVE_LAYOUT_ID}"), body),
    )
    .await;
    assert_eq!(updated.status(), StatusCode::OK);
}

fn stored_active_layout(rig: &Rig) -> SpatialLayout {
    layout_store::load(&rig.layouts_path())
        .expect("layout store should load")
        .remove(ACTIVE_LAYOUT_ID)
        .expect("active layout is stored")
}

fn strip_c_on_another_device() -> Output {
    let mut strip_c = strip("strip-c", "ch1");
    strip_c.device_id = "mock:other".to_owned();
    strip_c
}

#[tokio::test]
async fn activating_a_named_scene_realigns_its_primary_zone_to_the_active_layout() {
    let rig = rig_with_running_effect().await;
    let gaming = create_named_scene(&rig, "Gaming").await;
    studio_moves_strip_a(&rig).await;

    activate(&rig, &gaming).await;

    assert_eq!(
        strip_a_in_zone(&primary_zone(&rig.app).await),
        SAVED_POSITION,
        "a scene created before the Save shows the saved position, not its snapshot"
    );
}

#[tokio::test]
async fn a_save_in_another_scene_keeps_positions_saved_elsewhere() {
    let rig = rig_with_running_effect().await;
    let gaming = create_named_scene(&rig, "Gaming").await;
    studio_moves_strip_a(&rig).await;
    activate(&rig, &gaming).await;

    studio_moves(&rig, "ch2", (0.1, 0.9)).await;

    let published = rig.state.domains.layout.current();
    let stored = stored_active_layout(&rig);
    for layout in [&published, &stored] {
        assert_eq!(strip_a_in_layout(layout), SAVED_POSITION);
        assert_eq!(output_in_layout(layout, "strip-b"), (0.1, 0.9));
    }
}

#[tokio::test]
async fn a_save_in_a_stale_named_scene_keeps_outputs_only_the_active_layout_holds() {
    let rig = rig_with_running_effect().await;
    let gaming = create_named_scene(&rig, "Gaming").await;
    let mut zones = active_layout().zones;
    zones.push(strip_c_on_another_device());
    store_layout_edit(&rig, json!({ "zones": zones })).await;
    let (applied, _) = send(
        &rig.state,
        &rig.app,
        empty_request("POST", format!("/api/v1/layouts/{ACTIVE_LAYOUT_ID}/apply")),
    )
    .await;
    assert_eq!(applied.status(), StatusCode::OK);
    activate(&rig, &gaming).await;

    studio_moves_strip_a(&rig).await;

    let published = rig.state.domains.layout.current();
    let stored = stored_active_layout(&rig);
    for layout in [&published, &stored] {
        assert_eq!(output_ids(layout), vec!["strip-a", "strip-b", "strip-c"]);
        assert_eq!(strip_a_in_layout(layout), SAVED_POSITION);
    }
    let exclusions = std::fs::read_to_string(
        rig.tmp
            .path()
            .join("data")
            .join("layout-auto-exclusions.json"),
    )
    .unwrap_or_default();
    assert!(
        !exclusions.contains("mock:other"),
        "a Save never excludes a device from auto-layout: {exclusions}"
    );
}

#[tokio::test]
async fn a_save_keeps_stored_layout_edits_that_were_never_applied() {
    let rig = rig_with_running_effect().await;
    let mut zones = active_layout().zones;
    zones.push(strip_c_on_another_device());
    store_layout_edit(&rig, json!({ "name": "Renamed", "zones": zones })).await;

    studio_moves_strip_a(&rig).await;

    let stored = stored_active_layout(&rig);
    assert_eq!(stored.name, "Renamed");
    assert_eq!(output_ids(&stored), vec!["strip-a", "strip-b", "strip-c"]);
    assert_eq!(strip_a_in_layout(&stored), SAVED_POSITION);
    let published = rig.state.domains.layout.current();
    assert_eq!(published.name, "Active", "the stored edit stays unapplied");
    assert_eq!(output_ids(&published), vec!["strip-a", "strip-b"]);
    assert_eq!(strip_a_in_layout(&published), SAVED_POSITION);
}

#[tokio::test]
async fn a_scene_that_names_its_own_layout_takes_it_on_activation() {
    let rig = rig_with_running_effect().await;
    let gaming = create_named_scene(&rig, "Gaming").await;
    let mut own = active_layout();
    own.id = "layout-gaming".to_owned();
    own.name = "Gaming".to_owned();
    own.zones[0].position = NormalizedPosition::new(0.4, 0.6);
    rig.state
        .domains
        .layout
        .test_fixture()
        .catalog()
        .write()
        .await
        .insert(own.id.clone(), own.clone());
    let response = rig
        .app
        .clone()
        .oneshot(empty_request("GET", format!("/api/v1/scenes/{gaming}")))
        .await
        .expect("request should succeed");
    let mut document = body_json(response).await["data"].take();
    document["layout_id"] = json!(own.id);
    let (replaced, _) = send(
        &rig.state,
        &rig.app,
        json_request("PUT", format!("/api/v1/scenes/{gaming}"), document),
    )
    .await;
    assert_eq!(replaced.status(), StatusCode::OK);
    studio_moves_strip_a(&rig).await;

    activate(&rig, &gaming).await;

    assert_eq!(rig.state.domains.layout.current().id, own.id);
    assert_eq!(strip_a_in_zone(&primary_zone(&rig.app).await), (0.4, 0.6));
}

#[tokio::test]
async fn a_rejected_publication_leaves_the_save_undone() {
    let rig = rig_with_running_effect().await;
    let request = studio_save_request(&rig, "ch1", SAVED_POSITION).await;
    let work = rig.app.clone().oneshot(request);
    tokio::pin!(work);
    let executor = rig.state.layout_publication_test_executor();
    let rejected = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            tokio::select! {
                response = &mut work => break response.expect("request should succeed"),
                () = tokio::time::sleep(Duration::from_millis(1)) => {
                    executor.reject_next_layout_publication(
                        LayoutTransactionRejection::Superseded,
                    );
                }
            }
        }
    })
    .await
    .expect("the Save should answer");

    assert_eq!(rejected.status(), StatusCode::CONFLICT);
    assert_eq!(
        strip_a_in_zone(&primary_zone(&rig.app).await),
        AUTO_POSITION,
        "a Save that could not reach the active layout is not left half applied"
    );
    assert_eq!(
        strip_a_in_layout(&rig.state.domains.layout.current()),
        AUTO_POSITION
    );
    assert_eq!(
        strip_a_in_layout(&stored_active_layout(&rig)),
        AUTO_POSITION
    );
}

#[tokio::test]
async fn returning_to_the_default_scene_realigns_its_primary_zone() {
    let rig = rig_with_running_effect().await;
    let gaming = create_named_scene(&rig, "Gaming").await;
    activate(&rig, &gaming).await;
    studio_moves_strip_a(&rig).await;

    let (deactivated, _) = send(
        &rig.state,
        &rig.app,
        empty_request("POST", "/api/v1/scene/deactivate".into()),
    )
    .await;
    assert_eq!(deactivated.status(), StatusCode::OK);

    assert_eq!(
        strip_a_in_zone(&primary_zone(&rig.app).await),
        SAVED_POSITION,
        "the Default scene shows the placement saved while another scene was active"
    );
}

#[tokio::test]
async fn a_save_adopts_members_neither_layout_holds_yet() {
    let rig = rig_with_running_effect().await;
    // A member assigned in Studio lives in the primary zone only.
    let mut minted = strip("strip-m", "ch9");
    minted.device_id = "mock:minted".to_owned();
    minted.position = NormalizedPosition::new(0.6, 0.6);
    let mut zone_layout = active_layout();
    zone_layout.zones.push(minted);
    let glow = effect("Glow");
    register(&rig.state, &glow).await;
    let mut mutation = rig.state.scene_manager.begin_mutation().await;
    mutation
        .upsert_primary_zone(
            &glow,
            HashMap::new(),
            None,
            zone_layout,
            ChangeTrigger::System,
            None,
        )
        .expect("primary zone should take the minted member");
    hypercolor_daemon::domain::scene::commit_scene(&rig.state.domains.scene, mutation)
        .await
        .expect("seeded zone should commit");

    studio_moves_strip_a(&rig).await;

    let published = rig.state.domains.layout.current();
    let stored = stored_active_layout(&rig);
    for layout in [&published, &stored] {
        assert_eq!(output_ids(layout), vec!["strip-a", "strip-b", "strip-m"]);
        assert_eq!(output_in_layout(layout, "strip-m"), (0.6, 0.6));
    }
    apply_preset(&rig).await;
    let zone = primary_zone(&rig.app).await;
    assert_eq!(
        zone["members"].as_array().map(Vec::len),
        Some(3),
        "the adopted member survives the next apply"
    );
}
