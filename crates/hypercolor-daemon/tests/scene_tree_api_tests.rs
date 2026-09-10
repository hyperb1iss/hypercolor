//! Contract pins for the live scene tree at `/api/v1/scene` (Spec 78 §1).
//!
//! Four properties carry the design and are each fenced here: the
//! document always answers 200 and carries one revision token; layer
//! identity is minted, never reused, so a stale control patch 404s
//! instead of landing on a newer effect; structural writes honor an
//! optional `If-Match` while control writes deliberately do not; and a
//! control write to a bound key is a recoverable 409 rather than a
//! silent overwrite.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::SystemTime;

use axum::body::Body;
use http::{Request, StatusCode};
use hypercolor_core::asset::{AssetTypeHint, AssetUploadOptions};
use hypercolor_core::config::ConfigManager;
use hypercolor_core::effect::EffectEntry;
use hypercolor_daemon::api;
use hypercolor_daemon::app_state::AppState;
use hypercolor_types::api::output::OutputPowerMode;
use hypercolor_types::control::ControlValue;
use hypercolor_types::effect::{
    ControlBinding, ControlDefinition, ControlKind, ControlType, EffectCategory, EffectId,
    EffectMetadata, EffectSource, EffectState, PresetTemplate,
};
use hypercolor_types::event::{
    ChangeTrigger, EffectStopReason, HypercolorEvent, LayerStackChangeKind, SceneSettingsChangeKind,
};
use hypercolor_types::layer::LayerSource;
use hypercolor_types::library::PresetId;
use hypercolor_types::scene::ZoneId;
use hypercolor_types::spatial::{
    EdgeBehavior, LedTopology, NormalizedPosition, Output, SamplingMode, SpatialLayout,
    StripDirection,
};
use serde_json::json;
use tower::ServiceExt;
use uuid::Uuid;

static DATA_DIR_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

fn isolated_state() -> (Arc<AppState>, tempfile::TempDir) {
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

fn if_match(mut request: Request<Body>, revision: u64) -> Request<Body> {
    request.headers_mut().insert(
        http::header::IF_MATCH,
        http::HeaderValue::from_str(&format!("\"{revision}\"")).expect("valid etag"),
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

async fn insert_stream_asset(state: &Arc<AppState>, name: &str, url: &str) -> String {
    let mut options = AssetUploadOptions::new(name);
    options.type_hint = Some(AssetTypeHint::Stream);
    state
        .asset_library
        .write()
        .await
        .add_bytes(format!("{url}\n").as_bytes(), options)
        .expect("stream URL asset should upload")
        .record
        .id
        .to_string()
}

fn sample_output(id: &str, segment: Option<&str>) -> Output {
    Output {
        id: id.to_owned(),
        name: id.to_owned(),
        device_id: "mock:controller".to_owned(),
        zone_name: segment.map(ToOwned::to_owned),
        position: NormalizedPosition::new(0.25, 0.25),
        size: NormalizedPosition::new(0.5, 0.5),
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

fn sample_layout(outputs: Vec<Output>) -> SpatialLayout {
    SpatialLayout {
        id: "layout-test".to_owned(),
        name: "Layout".to_owned(),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones: outputs,
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        version: 1,
    }
}

/// Seed the live tree with a primary zone running one effect over two
/// member segments, which is the shape every test below reads back.
async fn seed_tree(state: &Arc<AppState>) -> EffectId {
    let metadata = sample_effect("Aurora");
    let effect_id = metadata.id;
    let _ = state
        .domains
        .effects
        .register(EffectEntry {
            metadata: metadata.clone(),
            source_path: "/tmp/aurora.rs".into(),
            modified: SystemTime::now(),
            state: EffectState::Loading,
        })
        .await;
    let mut mutation = state.scene_manager.begin_mutation().await;
    mutation
        .upsert_primary_zone(
            &metadata,
            HashMap::<String, ControlValue>::new(),
            None,
            sample_layout(vec![
                sample_output("out-a", Some("ch1")),
                sample_output("out-b", Some("ch2")),
            ]),
            hypercolor_types::event::ChangeTrigger::System,
            None,
        )
        .expect("primary zone should seed");
    hypercolor_daemon::domain::scene::commit_scene(&state.domains.scene, mutation)
        .await
        .expect("primary zone should commit");
    effect_id
}

async fn seed_display_zone(state: &Arc<AppState>, effect_id: EffectId) -> ZoneId {
    let metadata = state
        .domains
        .effects
        .metadata(effect_id)
        .await
        .expect("seeded effect");
    let device_id = hypercolor_types::device::DeviceId::new();
    let mut mutation = state.scene_manager.begin_mutation().await;
    let zone_id = mutation
        .upsert_display_zone(
            device_id,
            "Panel",
            &metadata,
            HashMap::new(),
            sample_layout(vec![sample_output("out-face", None)]),
            hypercolor_types::scene::DisplayFaceTarget::new(device_id),
        )
        .expect("face assigns")
        .id;
    hypercolor_daemon::domain::scene::commit_scene(&state.domains.scene, mutation)
        .await
        .expect("face should commit");
    zone_id
}

async fn read_document(app: &axum::Router) -> serde_json::Value {
    let response = send(app, empty_request("GET", "/api/v1/scene".into())).await;
    assert_eq!(response.status(), StatusCode::OK);
    body_json(response).await
}

fn primary_zone(document: &serde_json::Value) -> &serde_json::Value {
    document["data"]["zones"]
        .as_array()
        .expect("zones array")
        .iter()
        .find(|zone| zone["role"] == "primary")
        .expect("the live tree always carries a primary zone")
}

// ── The document ─────────────────────────────────────────────────────────

#[tokio::test]
async fn the_scene_document_always_answers_and_carries_one_revision() {
    let (state, _tmp) = isolated_state();
    let app = api::build_router(Arc::clone(&state), None);

    // No scene has ever been created, and the document still answers.
    let response = send(&app, empty_request("GET", "/api/v1/scene".into())).await;
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "an active scene always exists (Spec 78 §1.1)"
    );
    assert_eq!(response_etag(&response), "\"0\"");
    let document = body_json(response).await;

    assert_eq!(document["data"]["is_default"], true);
    assert_eq!(document["data"]["revision"], 0);
    for absent in ["groups_revision", "layers_version", "controls_version"] {
        assert!(
            document["data"].get(absent).is_none(),
            "{absent} is internal bookkeeping and never reaches the wire (Spec 78 §1.6)"
        );
    }
}

#[tokio::test]
async fn the_document_embeds_real_layer_identity_and_segment_members() {
    let (state, _tmp) = isolated_state();
    let app = api::build_router(Arc::clone(&state), None);
    seed_tree(&state).await;

    let document = read_document(&app).await;
    let zone = primary_zone(&document);

    let members = zone["members"].as_array().expect("members array");
    assert_eq!(members.len(), 2);
    assert_eq!(members[0]["segment"], "ch1");
    assert!(
        members[0].get("zone_name").is_none(),
        "device regions are segments on this surface (Spec 78 §5.1)"
    );
    assert!(
        members[0]["id"].is_string(),
        "membership identity is the resource id the member route addresses"
    );

    let layers = zone["layers"].as_array().expect("layers array");
    assert_eq!(layers.len(), 1);
    assert!(
        layers[0]["id"].is_string(),
        "the document embeds the layer id so no client ever synthesizes one"
    );

    let placements = zone["layout"]["placements"]
        .as_array()
        .expect("placements array");
    assert_eq!(placements.len(), 2);
    assert_eq!(placements[0]["member"], members[0]["id"]);
    assert!(
        placements[0].get("device_id").is_none(),
        "the layout contract speaks placements only (Spec 78 §1.2)"
    );
}

#[tokio::test]
async fn first_effect_apply_persists_a_fresh_real_layer_identity() {
    let (state, _tmp) = isolated_state();
    let metadata = sample_effect("First Light");
    let effect_id = metadata.id;
    let _ = state
        .domains
        .effects
        .register(EffectEntry {
            metadata,
            source_path: "/tmp/first-light.rs".into(),
            modified: SystemTime::now(),
            state: EffectState::Loading,
        })
        .await;
    let app = api::build_router(Arc::clone(&state), None);

    let response = send(
        &app,
        json_request(
            "POST",
            format!("/api/v1/effects/{effect_id}/apply"),
            json!({}),
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_etag(&response), "\"1\"");
    let applied = body_json(response).await;
    let zone_id = applied["data"]["zone"]["id"].as_str().expect("zone id");
    let layer_id = applied["data"]["zone"]["layers"][0]["id"]
        .as_str()
        .expect("layer id");
    assert_ne!(
        layer_id, zone_id,
        "a layer id is never derived from its zone"
    );

    let manager = state.scene_manager.snapshot().await;
    let zone = manager
        .active_scene()
        .and_then(hypercolor_types::scene::Scene::primary_zone)
        .expect("the first apply should persist a primary zone");
    let [layer] = zone.layers.as_slice() else {
        panic!("the first apply should persist exactly one real layer");
    };
    assert_eq!(layer.id.to_string(), layer_id);
}

// ── Layer identity lifecycle (§1.4) ──────────────────────────────────────

#[tokio::test]
async fn replacing_a_layer_mints_a_fresh_id_and_strands_the_old_one() {
    let (state, _tmp) = isolated_state();
    let app = api::build_router(Arc::clone(&state), None);
    let effect_id = seed_tree(&state).await;

    let document = read_document(&app).await;
    let zone = primary_zone(&document);
    let zone_id = zone["id"].as_str().expect("zone id").to_owned();
    let original_layer = zone["layers"][0]["id"]
        .as_str()
        .expect("layer id")
        .to_owned();
    hypercolor_daemon::domain::output::set_power(&state.domains.output, OutputPowerMode::Paused)
        .await;

    // Replace the layer with one running the very same effect. Spec 78
    // §1.4 mints a fresh id regardless: replacement is creation.
    let response = send(
        &app,
        json_request(
            "PUT",
            format!("/api/v1/scene/zones/{zone_id}/layers/{original_layer}"),
            json!({
                "source": { "type": "effect", "effect_id": effect_id, "controls": {} }
            }),
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        state.output_power.snapshot().manually_paused(),
        "whole-layer replacement must not wake paused output"
    );
    let replaced = body_json(response).await;
    let new_layer = replaced["data"]["layers"][0]["id"]
        .as_str()
        .expect("replacement layer id")
        .to_owned();

    assert_ne!(
        new_layer, original_layer,
        "an id never survives replacement, same effect or not (Spec 78 §1.4)"
    );

    // The stale id is gone, so a control patch aimed at it cannot land
    // on the effect that replaced it.
    let stale = send(
        &app,
        json_request(
            "PATCH",
            format!("/api/v1/scene/zones/{zone_id}/layers/{original_layer}/controls"),
            json!({ "values": { "speed": { "kind": "float", "value": 0.5 } } }),
        ),
    )
    .await;
    assert_eq!(
        stale.status(),
        StatusCode::NOT_FOUND,
        "a patch addressing a vanished layer 404s (Spec 78 §1.4)"
    );
    let body = body_json(stale).await;
    assert_eq!(body["error"]["code"], "layer_not_found");
    assert!(
        body["error"]["message"]
            .as_str()
            .expect("message")
            .starts_with("layer not found"),
        "the refusal names the layer, so a client knows to re-read /scene"
    );
}

// ── Concurrency split (§1.6) ─────────────────────────────────────────────

#[tokio::test]
async fn structural_writes_honor_if_match_and_control_writes_do_not() {
    let (state, _tmp) = isolated_state();
    let app = api::build_router(Arc::clone(&state), None);
    seed_tree(&state).await;

    let document = read_document(&app).await;
    let zone_id = primary_zone(&document)["id"]
        .as_str()
        .expect("zone id")
        .to_owned();
    let layer_id = primary_zone(&document)["layers"][0]["id"]
        .as_str()
        .expect("layer id")
        .to_owned();
    let revision = document["data"]["revision"].as_u64().expect("revision");

    // A structural write carrying a revision nobody has reached is a 412
    // naming the current one.
    let stale = send(
        &app,
        if_match(
            json_request(
                "POST",
                "/api/v1/scene/zones".into(),
                json!({ "name": "Desk" }),
            ),
            revision + 99,
        ),
    )
    .await;
    assert_eq!(stale.status(), StatusCode::PRECONDITION_FAILED);
    let body = body_json(stale).await;
    assert_eq!(body["error"]["code"], "precondition_failed");
    assert_eq!(body["error"]["details"]["current"], revision);

    // The same write with the revision it actually read lands.
    let fresh = send(
        &app,
        if_match(
            json_request(
                "POST",
                "/api/v1/scene/zones".into(),
                json!({ "name": "Desk" }),
            ),
            revision,
        ),
    )
    .await;
    assert_eq!(fresh.status(), StatusCode::CREATED);

    // Patching a zone is structural too, and it was the route most
    // likely to be missed: nothing about its body says "structural".
    let stale_zone_patch = send(
        &app,
        if_match(
            json_request(
                "PATCH",
                format!("/api/v1/scene/zones/{zone_id}"),
                json!({ "name": "Renamed" }),
            ),
            revision + 99,
        ),
    )
    .await;
    assert_eq!(
        stale_zone_patch.status(),
        StatusCode::PRECONDITION_FAILED,
        "a zone patch is a structural write (Spec 78 §1.6)"
    );

    // A control write is unguarded by contract: a slider drag would
    // self-invalidate every tick under a precondition, so the header is
    // not consulted even when the caller sends a stale one.
    let control = send(
        &app,
        if_match(
            json_request(
                "PATCH",
                format!("/api/v1/scene/zones/{zone_id}/layers/{layer_id}/controls"),
                json!({ "values": { "speed": { "kind": "float", "value": 0.5 } } }),
            ),
            revision + 99,
        ),
    )
    .await;
    assert_eq!(
        control.status(),
        StatusCode::OK,
        "value writes take no token (Spec 78 §1.6)"
    );
}

#[tokio::test]
async fn concurrent_control_writes_rebase_until_every_write_commits() {
    let (state, _tmp) = isolated_state();
    let app = api::build_router(Arc::clone(&state), None);
    seed_tree(&state).await;
    let before = read_document(&app).await;
    let revision = before["data"]["revision"].as_u64().expect("revision");
    let zone = primary_zone(&before);
    let zone_id = zone["id"].as_str().expect("zone id").to_owned();
    let layer_id = zone["layers"][0]["id"]
        .as_str()
        .expect("layer id")
        .to_owned();
    let writer_count = 12_usize;
    let barrier = Arc::new(tokio::sync::Barrier::new(writer_count + 1));
    let mut writers = Vec::with_capacity(writer_count);

    for index in 0..writer_count {
        let app = app.clone();
        let barrier = Arc::clone(&barrier);
        let route = format!("/api/v1/scene/zones/{zone_id}/layers/{layer_id}/controls");
        writers.push(tokio::spawn(async move {
            barrier.wait().await;
            let value = (index + 1) as f64 / (writer_count + 1) as f64;
            send(
                &app,
                json_request(
                    "PATCH",
                    route,
                    json!({ "values": { "speed": { "kind": "float", "value": value } } }),
                ),
            )
            .await
        }));
    }

    barrier.wait().await;
    for writer in writers {
        let response = writer.await.expect("control writer should join");
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "an unguarded control write rebases instead of surfacing a structural conflict"
        );
    }

    let after = read_document(&app).await;
    assert_eq!(
        after["data"]["revision"].as_u64(),
        Some(revision + writer_count as u64),
        "every admitted last-write-wins patch advances the tree once"
    );
}

#[tokio::test]
async fn effect_apply_sugars_reject_stale_revisions_before_waking_output() {
    let (state, _tmp) = isolated_state();
    let app = api::build_router(Arc::clone(&state), None);
    let effect_id = seed_tree(&state).await;
    let before = read_document(&app).await;
    let revision = before["data"]["revision"].as_u64().expect("revision");

    hypercolor_daemon::domain::output::set_power(&state.domains.output, OutputPowerMode::Paused)
        .await;

    let routes = [
        format!("/api/v1/effects/{effect_id}/apply"),
        format!(
            "/api/v1/effects/{effect_id}/presets/{}/apply",
            PresetId::stable("test-fast")
        ),
    ];
    for route in routes {
        let response = send(
            &app,
            if_match(json_request("POST", route, json!({})), revision + 1),
        )
        .await;
        assert_eq!(response.status(), StatusCode::PRECONDITION_FAILED);
        assert_eq!(
            body_json(response).await["error"]["code"],
            "precondition_failed"
        );
        assert!(
            state.output_power.snapshot().manually_paused(),
            "a rejected sugar must not wake output"
        );
        assert_eq!(
            read_document(&app).await["data"],
            before["data"],
            "a rejected sugar must not mutate the live scene"
        );
    }
}

#[tokio::test]
async fn preset_apply_uses_the_canonical_apply_body_without_discarding_fields() {
    let (state, _tmp) = isolated_state();
    let app = api::build_router(Arc::clone(&state), None);
    let effect_id = seed_tree(&state).await;
    let preset_id = PresetId::stable("test-fast");
    let before_revision = read_document(&app).await["data"]["revision"]
        .as_u64()
        .expect("live scene revision");
    let zone_id = state
        .scene_manager
        .snapshot()
        .await
        .active_scene()
        .and_then(hypercolor_types::scene::Scene::primary_zone)
        .expect("seeded scene should have a primary zone")
        .id;

    let response = send(
        &app,
        json_request(
            "POST",
            format!("/api/v1/effects/{effect_id}/presets/{preset_id}/apply"),
            json!({
                "controls": { "speed": { "kind": "float", "value": 0.7 } },
                "preset_id": PresetId::stable("body-value-must-not-win"),
                "zone": zone_id,
                "transition": { "type": "cut" }
            }),
        ),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response_etag(&response),
        format!("\"{}\"", before_revision + 1)
    );
    let body = body_json(response).await;
    assert_eq!(body["data"]["transition"]["type"], "cut");
    assert_eq!(
        body["data"]["zone"]["layers"][0]["source"]["preset_id"],
        preset_id.to_string()
    );
    let speed = &body["data"]["zone"]["layers"][0]["source"]["controls"]["speed"];
    assert_eq!(speed["kind"], "float");
    assert!((speed["value"].as_f64().expect("speed should be numeric") - 0.7).abs() < 1.0e-6);

    let rejected = send(
        &app,
        json_request(
            "POST",
            format!("/api/v1/effects/{effect_id}/presets/{preset_id}/apply"),
            json!({ "zone": zone_id, "discarded_field": true }),
        ),
    )
    .await;
    assert_eq!(rejected.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn a_control_patch_event_names_its_zone_and_real_layer() {
    let (state, _tmp) = isolated_state();
    let app = api::build_router(Arc::clone(&state), None);
    let effect_id = seed_tree(&state).await;
    let (zone_id, layer_id) = {
        let manager = state.scene_manager.snapshot().await;
        let zone = manager
            .active_scene()
            .and_then(hypercolor_types::scene::Scene::primary_zone)
            .expect("primary zone should exist");
        let layer_id = zone.layers.first().expect("effect layer should exist").id;
        (zone.id, layer_id)
    };
    let mut events = state.event_bus.subscribe_all();

    let response = send(
        &app,
        json_request(
            "PATCH",
            format!("/api/v1/scene/zones/{zone_id}/layers/{layer_id}/controls"),
            json!({ "values": { "speed": { "kind": "float", "value": 0.75 } } }),
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let control_event = std::iter::from_fn(|| events.try_recv().ok())
        .map(|timestamped| timestamped.event)
        .find_map(|event| match event {
            HypercolorEvent::EffectControlChanged {
                effect_id,
                control_id,
                old_value,
                new_value,
                zone_id,
                layer_id,
                trigger,
            } => Some((
                effect_id, control_id, old_value, new_value, zone_id, layer_id, trigger,
            )),
            _ => None,
        })
        .expect("the control patch should publish its addressed identity");

    assert_eq!(control_event.0, effect_id.to_string());
    assert_eq!(control_event.1, "speed");
    assert_eq!(control_event.2, ControlValue::Float(0.25));
    assert_eq!(control_event.3, ControlValue::Float(0.75));
    assert_eq!(control_event.4, zone_id);
    assert_eq!(control_event.5, layer_id);
    assert_eq!(control_event.6, ChangeTrigger::Api);
}

#[tokio::test]
async fn a_write_to_a_bound_control_is_refused_and_recoverable_in_one_request() {
    let (state, _tmp) = isolated_state();
    let app = api::build_router(Arc::clone(&state), None);
    seed_tree(&state).await;

    let document = read_document(&app).await;
    let zone_id = primary_zone(&document)["id"]
        .as_str()
        .expect("zone id")
        .to_owned();
    let layer_id = primary_zone(&document)["layers"][0]["id"]
        .as_str()
        .expect("layer id")
        .to_owned();

    {
        let mut mutation = state.scene_manager.begin_mutation().await;
        let zone_id = hypercolor_types::scene::ZoneId(zone_id.parse::<Uuid>().expect("zone uuid"));
        let (scene_id, layer_index, mut layer) = {
            let scene = mutation
                .scenes()
                .active_scene()
                .expect("seeded scene should remain active");
            let zone = scene
                .zones
                .iter()
                .find(|zone| zone.id == zone_id)
                .expect("seeded zone should remain active");
            let layer_index = zone
                .layers
                .iter()
                .position(|layer| layer.id.to_string() == layer_id)
                .expect("seeded layer should remain active");
            (scene.id, layer_index, zone.layers[layer_index].clone())
        };
        let LayerSource::Effect {
            control_bindings, ..
        } = &mut layer.source
        else {
            panic!("seeded layer should remain an effect layer");
        };
        control_bindings.insert(
            "speed".to_owned(),
            ControlBinding {
                sensor: "cpu".to_owned(),
                sensor_min: 0.0,
                sensor_max: 100.0,
                target_min: 0.0,
                target_max: 1.0,
                deadband: 0.0,
                smoothing: 0.0,
            },
        );
        mutation
            .replace_layer(scene_id, zone_id, layer.id, layer, layer_index)
            .expect("binding should attach");
        hypercolor_daemon::domain::scene::commit_scene(&state.domains.scene, mutation)
            .await
            .expect("binding should commit");
    }

    let refused = send(
        &app,
        json_request(
            "PATCH",
            format!("/api/v1/scene/zones/{zone_id}/layers/{layer_id}/controls"),
            json!({ "values": { "speed": { "kind": "float", "value": 0.5 } } }),
        ),
    )
    .await;
    assert_eq!(
        refused.status(),
        StatusCode::CONFLICT,
        "a manual write the next sensor resolution would overwrite is an error, not a race"
    );
    let body = body_json(refused).await;
    assert_eq!(body["error"]["code"], "control_bound");
    assert_eq!(body["error"]["details"]["bound"], json!(["speed"]));

    // The refusal is recoverable in the same shape: clearing the binding
    // and writing the value land in one commit.
    let recovered = send(
        &app,
        json_request(
            "PATCH",
            format!("/api/v1/scene/zones/{zone_id}/layers/{layer_id}/controls"),
            json!({
                "values": { "speed": { "kind": "float", "value": 0.5 } },
                "clear_bindings": ["speed"]
            }),
        ),
    )
    .await;
    assert_eq!(recovered.status(), StatusCode::OK);
    let zone = body_json(recovered).await;
    assert_eq!(
        zone["data"]["layers"][0]["source"]["controls"]["speed"],
        json!({ "kind": "float", "value": 0.5 })
    );
    assert!(
        zone["data"]["layers"][0]["source"]
            .get("control_bindings")
            .is_none(),
        "the cleared binding is gone, so the value is the caller's to own"
    );
}

// ── Zones, members, layout ───────────────────────────────────────────────

#[tokio::test]
async fn zone_lifecycle_moves_members_and_reports_the_new_revision() {
    let (state, _tmp) = isolated_state();
    let app = api::build_router(Arc::clone(&state), None);
    seed_tree(&state).await;

    let created = send(
        &app,
        json_request(
            "POST",
            "/api/v1/scene/zones".into(),
            json!({ "name": "Desk", "color": "#c084fc" }),
        ),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let after_create = response_etag(&created);
    let zone = body_json(created).await;
    let desk_id = zone["data"]["id"].as_str().expect("zone id").to_owned();
    assert_eq!(zone["data"]["role"], "custom");
    assert!(
        zone["data"]["members"]
            .as_array()
            .expect("members")
            .is_empty()
    );
    assert_eq!(
        zone["data"]["layout"],
        serde_json::Value::Null,
        "a zone with no members overrides no layout"
    );

    // Assign one segment by naming the device and the segment, never a
    // membership id the caller would have had to invent.
    let assigned = send(
        &app,
        json_request(
            "POST",
            format!("/api/v1/scene/zones/{desk_id}/members"),
            json!({ "device_id": "mock:controller", "segments": ["ch2"] }),
        ),
    )
    .await;
    assert_eq!(assigned.status(), StatusCode::OK);
    assert_ne!(
        response_etag(&assigned),
        after_create,
        "every structural write advances the one revision"
    );
    let desk = body_json(assigned).await;
    let members = desk["data"]["members"].as_array().expect("members");
    assert_eq!(members.len(), 1);
    assert_eq!(members[0]["segment"], "ch2");
    let member_id = members[0]["id"].as_str().expect("member id").to_owned();

    // The segment left the primary zone rather than being duplicated.
    let document = read_document(&app).await;
    let primary_members = primary_zone(&document)["members"]
        .as_array()
        .expect("members");
    assert_eq!(primary_members.len(), 1);
    assert_eq!(primary_members[0]["segment"], "ch1");

    // Reposition through the compact placement contract.
    let placed = send(
        &app,
        json_request(
            "PUT",
            format!("/api/v1/scene/zones/{desk_id}/layout"),
            json!({
                "placements": [{
                    "member": member_id,
                    "position": { "x": 0.75, "y": 0.5 },
                    "size": { "x": 0.2, "y": 0.1 },
                    "topology": { "type": "strip", "count": 4, "direction": "left_to_right" }
                }]
            }),
        ),
    )
    .await;
    assert_eq!(placed.status(), StatusCode::OK);
    let placed_zone = body_json(placed).await;
    assert_eq!(
        placed_zone["data"]["layout"]["placements"][0]["position"]["x"],
        json!(0.75)
    );

    // A placement naming a member the zone does not hold is refused
    // before anything moves.
    let rejected = send(
        &app,
        json_request(
            "PUT",
            format!("/api/v1/scene/zones/{desk_id}/layout"),
            json!({
                "placements": [{
                    "member": "out-a",
                    "position": { "x": 0.1, "y": 0.1 },
                    "size": { "x": 0.2, "y": 0.1 },
                    "topology": { "type": "strip", "count": 4, "direction": "left_to_right" }
                }]
            }),
        ),
    )
    .await;
    assert_eq!(rejected.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let unassigned = send(
        &app,
        empty_request(
            "DELETE",
            format!("/api/v1/scene/zones/{desk_id}/members/{member_id}"),
        ),
    )
    .await;
    assert_eq!(unassigned.status(), StatusCode::OK);
    let emptied = body_json(unassigned).await;
    assert!(
        emptied["data"]["members"]
            .as_array()
            .expect("members")
            .is_empty()
    );

    let deleted = send(
        &app,
        empty_request("DELETE", format!("/api/v1/scene/zones/{desk_id}")),
    )
    .await;
    assert_eq!(deleted.status(), StatusCode::OK);
    let document = body_json(deleted).await;
    assert!(
        document["data"]["zones"]
            .as_array()
            .expect("zones")
            .iter()
            .all(|zone| zone["id"] != desk_id.as_str()),
        "deleting a zone answers with the tree that no longer holds it"
    );
}

#[tokio::test]
async fn only_custom_zones_are_created_through_the_zone_route() {
    let (state, _tmp) = isolated_state();
    let app = api::build_router(Arc::clone(&state), None);

    for role in ["primary", "display"] {
        let response = send(
            &app,
            json_request(
                "POST",
                "/api/v1/scene/zones".into(),
                json!({ "name": "Nope", "role": role }),
            ),
        )
        .await;
        assert_eq!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "{role} zones are minted by the engine, not by this route"
        );
        let body = body_json(response).await;
        assert_eq!(body["error"]["code"], "validation_error");
        assert_eq!(body["error"]["details"]["field"], "role");
    }
}

// ── Layer stack ──────────────────────────────────────────────────────────

#[tokio::test]
async fn the_layer_stack_appends_reorders_and_drops() {
    let (state, _tmp) = isolated_state();
    let app = api::build_router(Arc::clone(&state), None);
    let effect_id = seed_tree(&state).await;

    let document = read_document(&app).await;
    let zone = primary_zone(&document);
    let zone_id = zone["id"].as_str().expect("zone id").to_owned();
    let first_layer = zone["layers"][0]["id"].as_str().expect("layer").to_owned();

    let created = send(
        &app,
        json_request(
            "POST",
            format!("/api/v1/scene/zones/{zone_id}/layers"),
            json!({
                "source": { "type": "effect", "effect_id": effect_id, "controls": {} },
                "name": "Overlay",
                "opacity": 0.5
            }),
        ),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let stacked = body_json(created).await;
    let layers = stacked["data"]["layers"].as_array().expect("layers");
    assert_eq!(layers.len(), 2);
    let second_layer = layers[1]["id"].as_str().expect("layer").to_owned();
    assert_ne!(second_layer, first_layer);

    let listed = send(
        &app,
        empty_request("GET", format!("/api/v1/scene/zones/{zone_id}/layers")),
    )
    .await;
    assert_eq!(listed.status(), StatusCode::OK);
    let list = body_json(listed).await;
    assert_eq!(list["data"]["total"], 2);
    assert!(
        list["data"]["page"].is_null(),
        "the stack is complete, so it fabricates no paging block"
    );

    let reordered = send(
        &app,
        json_request(
            "PATCH",
            format!("/api/v1/scene/zones/{zone_id}/layers/order"),
            json!({ "order": [second_layer, first_layer] }),
        ),
    )
    .await;
    assert_eq!(reordered.status(), StatusCode::OK);
    let flipped = body_json(reordered).await;
    assert_eq!(flipped["data"]["layers"][0]["id"], second_layer.as_str());

    // A partial order is a validation error, never a silent truncation.
    let partial = send(
        &app,
        json_request(
            "PATCH",
            format!("/api/v1/scene/zones/{zone_id}/layers/order"),
            json!({ "order": [second_layer] }),
        ),
    )
    .await;
    assert_eq!(partial.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let dropped = send(
        &app,
        empty_request(
            "DELETE",
            format!("/api/v1/scene/zones/{zone_id}/layers/{second_layer}"),
        ),
    )
    .await;
    assert_eq!(dropped.status(), StatusCode::OK);
    let remaining = body_json(dropped).await;
    assert_eq!(
        remaining["data"]["layers"]
            .as_array()
            .expect("layers")
            .len(),
        1
    );
}

// ── Scene-level gestures ─────────────────────────────────────────────────

#[tokio::test]
async fn clear_empties_one_zone_or_the_whole_tree() {
    let (state, _tmp) = isolated_state();
    let app = api::build_router(Arc::clone(&state), None);
    seed_tree(&state).await;

    let document = read_document(&app).await;
    let zone_id = primary_zone(&document)["id"]
        .as_str()
        .expect("zone id")
        .to_owned();

    let cleared = send(
        &app,
        json_request(
            "POST",
            "/api/v1/scene/clear".into(),
            json!({ "zone": zone_id }),
        ),
    )
    .await;
    assert_eq!(cleared.status(), StatusCode::OK);
    let after = body_json(cleared).await;
    assert!(
        primary_zone(&after)["layers"]
            .as_array()
            .expect("layers")
            .is_empty(),
        "clearing a zone empties its stack"
    );

    // The bodyless form is the whole-tree stop gesture.
    let stopped = send(&app, empty_request("POST", "/api/v1/scene/clear".into())).await;
    assert_eq!(stopped.status(), StatusCode::OK);
}

#[tokio::test]
async fn whole_tree_clear_publishes_the_destructive_stop_lifecycle() {
    let (state, _tmp) = isolated_state();
    let app = api::build_router(Arc::clone(&state), None);
    seed_tree(&state).await;
    let before = read_document(&app).await;
    let expected_zone_id = primary_zone(&before)["id"]
        .as_str()
        .expect("zone id")
        .to_owned();
    let expected_zone_name = primary_zone(&before)["name"]
        .as_str()
        .expect("zone name")
        .to_owned();
    let mut events = state.event_bus.subscribe_all();

    let response = send(&app, empty_request("POST", "/api/v1/scene/clear".into())).await;
    assert_eq!(response.status(), StatusCode::OK);

    let stopped = std::iter::from_fn(|| events.try_recv().ok())
        .find_map(|timestamped| match timestamped.event {
            HypercolorEvent::EffectStopped {
                reason,
                zone_id,
                zone_name,
                ..
            } => Some((reason, zone_id, zone_name)),
            _ => None,
        })
        .expect("the whole-tree stop gesture should publish effect_stopped");
    assert_eq!(stopped.0, EffectStopReason::Stopped);
    assert_eq!(
        stopped.1.map(|zone_id| zone_id.to_string()).as_deref(),
        Some(expected_zone_id.as_str())
    );
    assert_eq!(stopped.2.as_deref(), Some(expected_zone_name.as_str()));
}

#[tokio::test]
async fn patching_the_scene_refuses_to_rename_the_default() {
    let (state, _tmp) = isolated_state();
    let app = api::build_router(Arc::clone(&state), None);

    let renamed = send(
        &app,
        json_request(
            "PATCH",
            "/api/v1/scene".into(),
            json!({ "name": "Not Allowed" }),
        ),
    )
    .await;
    assert_eq!(renamed.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = body_json(renamed).await;
    assert_eq!(body["error"]["details"]["field"], "name");

    // The policy field on the same resource still patches.
    let policy = send(
        &app,
        json_request(
            "PATCH",
            "/api/v1/scene".into(),
            json!({ "unassigned_behavior": "off" }),
        ),
    )
    .await;
    assert_eq!(policy.status(), StatusCode::OK);
    let document = body_json(policy).await;
    assert_eq!(document["data"]["unassigned_behavior"], "off");
}

#[tokio::test]
async fn a_typo_in_a_request_body_is_a_loud_rejection() {
    let (state, _tmp) = isolated_state();
    let app = api::build_router(Arc::clone(&state), None);

    let response = send(
        &app,
        json_request(
            "POST",
            "/api/v1/scene/zones".into(),
            json!({ "name": "Desk", "colour": "#fff" }),
        ),
    )
    .await;
    assert!(
        response.status().is_client_error(),
        "an unknown field is refused rather than silently dropped"
    );
}

#[tokio::test]
async fn an_unknown_zone_is_a_not_found_not_a_panic() {
    let (state, _tmp) = isolated_state();
    let app = api::build_router(Arc::clone(&state), None);

    for uri in [
        "/api/v1/scene/zones/not-a-uuid".to_owned(),
        format!("/api/v1/scene/zones/{}", Uuid::now_v7()),
    ] {
        let response = send(&app, empty_request("GET", uri.clone())).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{uri}");
        let body = body_json(response).await;
        assert_eq!(body["error"]["code"], "zone_not_found");
    }
}

// ── Findings the first adversarial pass surfaced ─────────────────────────

#[tokio::test]
async fn every_zone_write_echoes_the_advanced_revision() {
    let (state, _tmp) = isolated_state();
    let app = api::build_router(Arc::clone(&state), None);
    seed_tree(&state).await;

    let document = read_document(&app).await;
    let zone_id = primary_zone(&document)["id"]
        .as_str()
        .expect("zone id")
        .to_owned();
    let before = document["data"]["revision"].as_u64().expect("revision");

    let patched = send(
        &app,
        json_request(
            "PATCH",
            format!("/api/v1/scene/zones/{zone_id}"),
            json!({ "brightness": 0.5 }),
        ),
    )
    .await;
    assert_eq!(patched.status(), StatusCode::OK);
    assert_eq!(
        response_etag(&patched),
        format!("\"{}\"", before + 1),
        "a caller learns the new token from the write it just made"
    );
}

#[tokio::test]
async fn a_control_patch_is_validated_against_the_effect_schema() {
    let (state, _tmp) = isolated_state();
    let app = api::build_router(Arc::clone(&state), None);
    seed_tree(&state).await;

    let document = read_document(&app).await;
    let zone_id = primary_zone(&document)["id"]
        .as_str()
        .expect("zone id")
        .to_owned();
    let layer_id = primary_zone(&document)["layers"][0]["id"]
        .as_str()
        .expect("layer id")
        .to_owned();

    let empty = send(
        &app,
        json_request(
            "PATCH",
            format!("/api/v1/scene/zones/{zone_id}/layers/{layer_id}/controls"),
            json!({ "values": {} }),
        ),
    )
    .await;
    assert_eq!(
        empty.status(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "a patch that writes nothing must not advance the revision"
    );
}

#[tokio::test]
async fn clearing_the_tree_leaves_display_faces_alone() {
    let (state, _tmp) = isolated_state();
    let app = api::build_router(Arc::clone(&state), None);
    let effect_id = seed_tree(&state).await;

    let display_zone_id = {
        let metadata = state
            .domains
            .effects
            .metadata(effect_id)
            .await
            .expect("seeded effect");
        let device_id = hypercolor_types::device::DeviceId::new();
        let mut mutation = state.scene_manager.begin_mutation().await;
        let zone_id = mutation
            .upsert_display_zone(
                device_id,
                "Panel",
                &metadata,
                HashMap::<String, ControlValue>::new(),
                sample_layout(vec![sample_output("out-face", None)]),
                hypercolor_types::scene::DisplayFaceTarget::new(device_id),
            )
            .expect("face assigns")
            .id;
        hypercolor_daemon::domain::scene::commit_scene(&state.domains.scene, mutation)
            .await
            .expect("face should commit");
        zone_id
    };

    let cleared = send(&app, empty_request("POST", "/api/v1/scene/clear".into())).await;
    assert_eq!(cleared.status(), StatusCode::OK);
    let document = body_json(cleared).await;
    let face = document["data"]["zones"]
        .as_array()
        .expect("zones")
        .iter()
        .find(|zone| zone["id"] == display_zone_id.to_string().as_str())
        .expect("the display zone survives");
    assert!(
        !face["layers"].as_array().expect("layers").is_empty(),
        "faces are owned by /displays, so the stop gesture never blanks one (Spec 78 §1.3)"
    );

    let targeted = send(
        &app,
        json_request(
            "POST",
            "/api/v1/scene/clear".into(),
            json!({ "zone": display_zone_id.to_string() }),
        ),
    )
    .await;
    assert_eq!(
        targeted.status(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "and naming one explicitly is refused rather than honored"
    );
}

#[tokio::test]
async fn generic_live_tree_structure_mutations_cannot_edit_display_owned_zones() {
    let (state, _tmp) = isolated_state();
    let app = api::build_router(Arc::clone(&state), None);
    let effect_id = seed_tree(&state).await;
    let display_zone_id = seed_display_zone(&state, effect_id).await;
    let before = read_document(&app).await;
    let face = before["data"]["zones"]
        .as_array()
        .expect("zones")
        .iter()
        .find(|zone| zone["id"] == display_zone_id.to_string())
        .expect("display zone");
    let member_id = face["members"][0]["id"]
        .as_str()
        .expect("face member")
        .to_owned();
    let placements = face["layout"]["placements"].clone();
    let zone = display_zone_id.to_string();
    let requests = vec![
        json_request(
            "PATCH",
            format!("/api/v1/scene/zones/{zone}"),
            json!({ "name": "Hijacked" }),
        ),
        empty_request("DELETE", format!("/api/v1/scene/zones/{zone}")),
        json_request(
            "PUT",
            format!("/api/v1/scene/zones/{zone}/layout"),
            json!({ "placements": placements }),
        ),
        json_request(
            "POST",
            format!("/api/v1/scene/zones/{zone}/members"),
            json!({ "device_id": "mock:controller", "segments": [] }),
        ),
        empty_request(
            "DELETE",
            format!("/api/v1/scene/zones/{zone}/members/{member_id}"),
        ),
    ];

    for request in requests {
        let response = send(&app, request).await;
        assert_eq!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "display zone structure belongs exclusively to the display API"
        );
    }

    let after = read_document(&app).await;
    assert_eq!(after["data"], before["data"]);
}

#[tokio::test]
async fn display_role_zones_accept_live_layer_stack_mutations() {
    let (state, _tmp) = isolated_state();
    let app = api::build_router(Arc::clone(&state), None);
    let effect_id = seed_tree(&state).await;
    let zone_id = seed_display_zone(&state, effect_id).await;
    let before = read_document(&app).await;
    let original_layer = before["data"]["zones"]
        .as_array()
        .expect("zones")
        .iter()
        .find(|zone| zone["id"] == zone_id.to_string())
        .expect("display zone")["layers"][0]["id"]
        .as_str()
        .expect("face layer")
        .to_owned();

    let created = send(
        &app,
        json_request(
            "POST",
            format!("/api/v1/scene/zones/{zone_id}/layers"),
            json!({
                "source": { "type": "effect", "effect_id": effect_id, "controls": {} }
            }),
        ),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let created = body_json(created).await;
    let added_layer = created["data"]["layers"]
        .as_array()
        .expect("layers")
        .iter()
        .find(|layer| layer["id"] != original_layer)
        .expect("added layer")["id"]
        .as_str()
        .expect("added layer id")
        .to_owned();

    let reordered = send(
        &app,
        json_request(
            "PATCH",
            format!("/api/v1/scene/zones/{zone_id}/layers/order"),
            json!({ "order": [added_layer.clone(), original_layer.clone()] }),
        ),
    )
    .await;
    assert_eq!(reordered.status(), StatusCode::OK);

    let replaced = send(
        &app,
        json_request(
            "PUT",
            format!("/api/v1/scene/zones/{zone_id}/layers/{original_layer}"),
            json!({
                "source": { "type": "effect", "effect_id": effect_id, "controls": {} }
            }),
        ),
    )
    .await;
    assert_eq!(replaced.status(), StatusCode::OK);
    let replaced = body_json(replaced).await;
    let replacement = replaced["data"]["layers"]
        .as_array()
        .expect("layers")
        .iter()
        .find(|layer| layer["id"] != added_layer)
        .expect("replacement layer")["id"]
        .as_str()
        .expect("replacement layer id")
        .to_owned();
    assert_ne!(replacement, original_layer);

    let patched = send(
        &app,
        json_request(
            "PATCH",
            format!("/api/v1/scene/zones/{zone_id}/layers/{replacement}/controls"),
            json!({ "values": { "speed": { "kind": "float", "value": 0.75 } } }),
        ),
    )
    .await;
    assert_eq!(patched.status(), StatusCode::OK);

    let deleted = send(
        &app,
        empty_request(
            "DELETE",
            format!("/api/v1/scene/zones/{zone_id}/layers/{added_layer}"),
        ),
    )
    .await;
    assert_eq!(deleted.status(), StatusCode::OK);
    let deleted = body_json(deleted).await;
    assert_eq!(
        deleted["data"]["layers"].as_array().expect("layers").len(),
        1
    );
}

#[tokio::test]
async fn a_layout_mismatch_names_the_mismatch_rather_than_the_zone() {
    let (state, _tmp) = isolated_state();
    let app = api::build_router(Arc::clone(&state), None);
    seed_tree(&state).await;

    let document = read_document(&app).await;
    let zone = primary_zone(&document);
    let zone_id = zone["id"].as_str().expect("zone id").to_owned();
    let member = zone["members"][0]["id"]
        .as_str()
        .expect("member")
        .to_owned();
    let topology = zone["layout"]["placements"][0]["topology"].clone();

    // The same member twice passes a naive length check but is not the
    // zone's member set.
    let duplicated = send(
        &app,
        json_request(
            "PUT",
            format!("/api/v1/scene/zones/{zone_id}/layout"),
            json!({
                "placements": [
                    { "member": member, "position": { "x": 0.1, "y": 0.1 },
                      "size": { "x": 0.2, "y": 0.1 }, "topology": topology },
                    { "member": member, "position": { "x": 0.2, "y": 0.2 },
                      "size": { "x": 0.2, "y": 0.1 }, "topology": topology }
                ]
            }),
        ),
    )
    .await;
    assert_eq!(duplicated.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // Topology is hardware, not placement.
    let retopologized = send(
        &app,
        json_request(
            "PUT",
            format!("/api/v1/scene/zones/{zone_id}/layout"),
            json!({
                "placements": [
                    { "member": member, "position": { "x": 0.1, "y": 0.1 },
                      "size": { "x": 0.2, "y": 0.1 },
                      "topology": { "type": "strip", "count": 99, "direction": "left_to_right" } },
                    { "member": zone["members"][1]["id"], "position": { "x": 0.3, "y": 0.3 },
                      "size": { "x": 0.2, "y": 0.1 }, "topology": topology }
                ]
            }),
        ),
    )
    .await;
    assert_eq!(retopologized.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn omitting_segments_is_refused_on_multi_segment_hardware() {
    let (state, _tmp) = isolated_state();
    let app = api::build_router(Arc::clone(&state), None);
    seed_tree(&state).await;

    let created = send(
        &app,
        json_request(
            "POST",
            "/api/v1/scene/zones".into(),
            json!({ "name": "Desk" }),
        ),
    )
    .await;
    let zone = body_json(created).await;
    let desk_id = zone["data"]["id"].as_str().expect("zone id").to_owned();

    let response = send(
        &app,
        json_request(
            "POST",
            format!("/api/v1/scene/zones/{desk_id}/members"),
            json!({ "device_id": "mock:controller" }),
        ),
    )
    .await;
    assert_eq!(
        response.status(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "omitting segments means the whole device, which only reads unambiguously on single-segment hardware"
    );
    let body = body_json(response).await;
    assert_eq!(body["error"]["details"]["segments"], json!(["ch1", "ch2"]));
}

#[tokio::test]
async fn reading_the_tree_never_advances_the_revision() {
    let (state, _tmp) = isolated_state();
    let app = api::build_router(Arc::clone(&state), None);
    seed_tree(&state).await;

    let first = read_document(&app).await["data"]["revision"]
        .as_u64()
        .expect("revision");
    for _ in 0..3 {
        let _ = read_document(&app).await;
    }
    let last = read_document(&app).await["data"]["revision"]
        .as_u64()
        .expect("revision");
    assert_eq!(first, last, "a safe method must not commit");
}

#[tokio::test]
async fn live_layer_create_and_replace_enforce_media_admission() {
    let (state, _tmp) = isolated_state();
    let app = api::build_router(Arc::clone(&state), None);
    seed_tree(&state).await;
    let first_stream =
        insert_stream_asset(&state, "camera-a.stream", "https://1.1.1.1/live-a.m3u8").await;
    let second_stream =
        insert_stream_asset(&state, "camera-b.stream", "https://8.8.8.8/live-b.m3u8").await;

    let document = read_document(&app).await;
    let zone = primary_zone(&document);
    let zone_id = zone["id"].as_str().expect("zone id").to_owned();
    let original_layer = zone["layers"][0]["id"]
        .as_str()
        .expect("layer id")
        .to_owned();

    let admitted = send(
        &app,
        json_request(
            "POST",
            format!("/api/v1/scene/zones/{zone_id}/layers"),
            json!({ "source": { "type": "media", "asset_id": first_stream } }),
        ),
    )
    .await;
    assert_eq!(admitted.status(), StatusCode::CREATED);

    let create_rejected = send(
        &app,
        json_request(
            "POST",
            format!("/api/v1/scene/zones/{zone_id}/layers"),
            json!({ "source": { "type": "media", "asset_id": second_stream } }),
        ),
    )
    .await;
    assert_eq!(create_rejected.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let create_error = body_json(create_rejected).await;
    assert_eq!(create_error["error"]["details"]["counts"]["livestream"], 2);
    assert_eq!(create_error["error"]["details"]["caps"]["livestream"], 1);

    let replace_rejected = send(
        &app,
        json_request(
            "PUT",
            format!("/api/v1/scene/zones/{zone_id}/layers/{original_layer}"),
            json!({ "source": { "type": "media", "asset_id": second_stream } }),
        ),
    )
    .await;
    assert_eq!(replace_rejected.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let replace_error = body_json(replace_rejected).await;
    assert_eq!(replace_error["error"]["details"]["counts"]["livestream"], 2);

    let unchanged = read_document(&app).await;
    assert!(
        primary_zone(&unchanged)["layers"]
            .as_array()
            .expect("layers")
            .iter()
            .any(|layer| layer["id"] == original_layer),
        "a refused replacement leaves the addressed layer intact"
    );
}

#[tokio::test]
async fn live_layer_replacement_and_controls_publish_stack_events() {
    let (state, _tmp) = isolated_state();
    let app = api::build_router(Arc::clone(&state), None);
    let effect_id = seed_tree(&state).await;
    let document = read_document(&app).await;
    let zone = primary_zone(&document);
    let zone_id = zone["id"].as_str().expect("zone id").to_owned();
    let original_layer = zone["layers"][0]["id"]
        .as_str()
        .expect("layer id")
        .to_owned();
    let mut events = state.event_bus.subscribe_all();

    let replaced = send(
        &app,
        json_request(
            "PUT",
            format!("/api/v1/scene/zones/{zone_id}/layers/{original_layer}"),
            json!({
                "source": { "type": "effect", "effect_id": effect_id, "controls": {} }
            }),
        ),
    )
    .await;
    assert_eq!(replaced.status(), StatusCode::OK);
    let replaced = body_json(replaced).await;
    let replacement = replaced["data"]["layers"][0]["id"]
        .as_str()
        .expect("replacement layer")
        .to_owned();

    let event = events.recv().await.expect("zone event");
    assert!(matches!(event.event, HypercolorEvent::ZoneChanged { .. }));
    let event = events.recv().await.expect("layer event");
    assert!(matches!(
        event.event,
        HypercolorEvent::LayerStackChanged {
            kind: LayerStackChangeKind::Updated,
            ..
        }
    ));

    let patched = send(
        &app,
        json_request(
            "PATCH",
            format!("/api/v1/scene/zones/{zone_id}/layers/{replacement}/controls"),
            json!({ "values": { "speed": { "kind": "float", "value": 0.75 } } }),
        ),
    )
    .await;
    assert_eq!(patched.status(), StatusCode::OK);

    let event = events.recv().await.expect("zone controls event");
    assert!(matches!(event.event, HypercolorEvent::ZoneChanged { .. }));
    let event = events.recv().await.expect("effect control event");
    assert!(matches!(
        event.event,
        HypercolorEvent::EffectControlChanged { .. }
    ));
    let event = events.recv().await.expect("layer controls event");
    assert!(matches!(
        event.event,
        HypercolorEvent::LayerStackChanged {
            kind: LayerStackChangeKind::ControlsPatched,
            ..
        }
    ));
}

#[tokio::test]
async fn scene_settings_event_carries_the_candidate_revision() {
    let (state, _tmp) = isolated_state();
    let app = api::build_router(Arc::clone(&state), None);
    seed_tree(&state).await;
    let mut events = state.event_bus.subscribe_all();

    let patched = send(
        &app,
        json_request(
            "PATCH",
            "/api/v1/scene".into(),
            json!({ "unassigned_behavior": "off" }),
        ),
    )
    .await;
    assert_eq!(patched.status(), StatusCode::OK);
    let expected = body_json(patched).await["data"]["revision"]
        .as_u64()
        .expect("scene revision");

    let event = events.recv().await.expect("scene settings event");
    assert!(matches!(
        event.event,
        HypercolorEvent::SceneSettingsChanged {
            revision,
            kind: SceneSettingsChangeKind::UnassignedBehavior,
            ..
        } if revision == expected
    ));
}

#[tokio::test]
async fn membership_edit_restores_offline_outputs_and_preserves_hidden_fields() {
    use hypercolor_types::api::scene::{
        EditMembersRequest, EditMembersResponse, MemberEdit, MemberState,
    };
    let (state, _tmp) = isolated_state();
    let app = api::build_router(Arc::clone(&state), None);
    seed_tree(&state).await;
    let document = read_document(&app).await;
    let scene_id = serde_json::from_value(document["data"]["id"].clone()).expect("scene id");
    let zone_id = serde_json::from_value(primary_zone(&document)["id"].clone()).expect("zone id");
    let mut projected_output = sample_output("out-a", Some("ch1"));
    // The compact scene document intentionally does not expose these fields.
    projected_output.sampling_mode = None;
    projected_output.edge_behavior = None;
    let request = EditMembersRequest {
        assignment: None,
        scene_id,
        changes: vec![MemberEdit {
            before: Some(MemberState {
                zone_id,
                output: projected_output,
                index: 0,
            }),
            after: None,
        }],
    };
    let revision = document["data"]["revision"].as_u64().expect("revision");
    let removed = send(
        &app,
        if_match(
            json_request("POST", "/api/v1/scene/members/edit".into(), json!(request)),
            revision,
        ),
    )
    .await;
    assert_eq!(removed.status(), StatusCode::OK);
    let removed: EditMembersResponse =
        serde_json::from_value(body_json(removed).await["data"].clone()).expect("receipt");
    assert_eq!(removed.document.revision, revision + 1);
    let canonical = removed.changes[0]
        .before
        .as_ref()
        .expect("canonical output");
    assert_eq!(canonical.output.sampling_mode, Some(SamplingMode::Bilinear));
    assert_eq!(canonical.output.edge_behavior, Some(EdgeBehavior::Clamp));
    assert_eq!(removed.document.zones[0].members.len(), 1);
    let restore = EditMembersRequest {
        assignment: None,
        scene_id,
        changes: removed
            .changes
            .iter()
            .map(|change| MemberEdit {
                before: change.after.clone(),
                after: change.before.clone(),
            })
            .collect(),
    };
    let restored = send(
        &app,
        if_match(
            json_request("POST", "/api/v1/scene/members/edit".into(), json!(restore)),
            removed.document.revision,
        ),
    )
    .await;
    assert_eq!(restored.status(), StatusCode::OK);
    let restored: EditMembersResponse =
        serde_json::from_value(body_json(restored).await["data"].clone()).expect("receipt");
    assert_eq!(restored.document.zones[0].members[0].id.0, "out-a");
    assert_eq!(restored.document.zones[0].members[1].id.0, "out-b");
    assert_eq!(
        restored.document.zones[0].layers,
        removed.document.zones[0].layers
    );
    assert_eq!(restored.changes[0].after, removed.changes[0].before);
}

#[tokio::test]
async fn membership_edit_rejects_stale_invalid_and_partial_batches() {
    use hypercolor_types::api::scene::{EditMembersRequest, MemberEdit, MemberState};
    let (state, _tmp) = isolated_state();
    let app = api::build_router(Arc::clone(&state), None);
    seed_tree(&state).await;
    let document = read_document(&app).await;
    let scene_id = serde_json::from_value(document["data"]["id"].clone()).expect("scene id");
    let zone_id = serde_json::from_value(primary_zone(&document)["id"].clone()).expect("zone id");
    let revision = document["data"]["revision"].as_u64().expect("revision");
    let valid = MemberEdit {
        before: Some(MemberState {
            zone_id,
            output: sample_output("out-a", Some("ch1")),
            index: 0,
        }),
        after: None,
    };
    let mut invalid_output = sample_output("out-b", Some("ch2"));
    invalid_output.position.x = 0.99;
    let request = EditMembersRequest {
        assignment: None,
        scene_id,
        changes: vec![
            valid.clone(),
            MemberEdit {
                before: Some(MemberState {
                    zone_id,
                    output: invalid_output,
                    index: 1,
                }),
                after: None,
            },
        ],
    };
    let rejected = send(
        &app,
        if_match(
            json_request("POST", "/api/v1/scene/members/edit".into(), json!(request)),
            revision,
        ),
    )
    .await;
    assert_eq!(rejected.status(), StatusCode::CONFLICT);
    let after = read_document(&app).await;
    assert_eq!(
        after["data"], document["data"],
        "invalid second entry must leave first member intact"
    );
    let request = EditMembersRequest {
        assignment: None,
        scene_id,
        changes: vec![valid],
    };
    let stale = send(
        &app,
        if_match(
            json_request("POST", "/api/v1/scene/members/edit".into(), json!(request)),
            revision - 1,
        ),
    )
    .await;
    assert_eq!(stale.status(), StatusCode::PRECONDITION_FAILED);
    let missing = send(
        &app,
        json_request("POST", "/api/v1/scene/members/edit".into(), json!(request)),
    )
    .await;
    assert_eq!(missing.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let mut wrong_scene = request;
    wrong_scene.scene_id = hypercolor_types::scene::SceneId::new();
    let rejected = send(
        &app,
        if_match(
            json_request(
                "POST",
                "/api/v1/scene/members/edit".into(),
                json!(wrong_scene),
            ),
            revision,
        ),
    )
    .await;
    assert_eq!(rejected.status(), StatusCode::CONFLICT);
    assert_eq!(read_document(&app).await["data"], document["data"]);
}

#[tokio::test]
async fn membership_edit_moves_outputs_without_losing_unrelated_controls() {
    use hypercolor_types::api::scene::{
        EditMembersRequest, EditMembersResponse, MemberEdit, MemberState,
    };
    let (state, _tmp) = isolated_state();
    let app = api::build_router(Arc::clone(&state), None);
    seed_tree(&state).await;
    let created = send(
        &app,
        json_request(
            "POST",
            "/api/v1/scene/zones".into(),
            json!({"name": "Desk"}),
        ),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let desk_id =
        serde_json::from_value(body_json(created).await["data"]["id"].clone()).expect("desk id");
    let document = read_document(&app).await;
    let scene_id = serde_json::from_value(document["data"]["id"].clone()).expect("scene id");
    let zone_id = serde_json::from_value(primary_zone(&document)["id"].clone()).expect("zone id");
    let before = MemberState {
        zone_id,
        output: sample_output("out-a", Some("ch1")),
        index: 0,
    };
    let mut after = before.clone();
    after.zone_id = desk_id;
    after.output.position.x = 0.75;
    after.output.sampling_mode = None;
    after.output.edge_behavior = None;
    let moved = send(
        &app,
        if_match(
            json_request(
                "POST",
                "/api/v1/scene/members/edit".into(),
                json!(EditMembersRequest {
                    assignment: None,
                    scene_id,
                    changes: vec![MemberEdit {
                        before: Some(before),
                        after: Some(after)
                    }],
                }),
            ),
            document["data"]["revision"].as_u64().expect("revision"),
        ),
    )
    .await;
    assert_eq!(moved.status(), StatusCode::OK);
    let moved: EditMembersResponse =
        serde_json::from_value(body_json(moved).await["data"].clone()).expect("receipt");
    let after = moved.changes[0].after.as_ref().expect("moved output");
    assert_eq!(after.output.position.x, 0.75);
    assert_eq!(after.output.sampling_mode, Some(SamplingMode::Bilinear));
    let layer_id = moved.document.zones[0].layers[0].id;
    let patched = send(
        &app,
        json_request(
            "PATCH",
            format!("/api/v1/scene/zones/{zone_id}/layers/{layer_id}/controls"),
            json!({"values": {"speed": {"kind": "float", "value": 1.5}}}),
        ),
    )
    .await;
    assert_eq!(patched.status(), StatusCode::OK);
    let patched = read_document(&app).await;
    let undo = EditMembersRequest {
        assignment: None,
        scene_id,
        changes: moved
            .changes
            .iter()
            .map(|change| MemberEdit {
                before: change.after.clone(),
                after: change.before.clone(),
            })
            .collect(),
    };
    let restored = send(
        &app,
        if_match(
            json_request("POST", "/api/v1/scene/members/edit".into(), json!(undo)),
            patched["data"]["revision"].as_u64().expect("revision"),
        ),
    )
    .await;
    assert_eq!(restored.status(), StatusCode::OK);
    let restored = body_json(restored).await;
    assert_eq!(
        restored["data"]["document"]["zones"][0]["layers"],
        patched["data"]["zones"][0]["layers"]
    );
    assert_eq!(
        restored["data"]["document"]["zones"][0]["members"][0]["id"],
        "out-a"
    );
    assert_eq!(
        restored["data"]["document"]["zones"][0]["layout"]["placements"][0]["position"],
        document["data"]["zones"][0]["layout"]["placements"][0]["position"]
    );
}

#[tokio::test]
async fn membership_edit_rejects_duplicate_identities_geometry_and_display_targets() {
    use hypercolor_types::api::scene::{EditMembersRequest, MemberEdit, MemberState};
    let (state, _tmp) = isolated_state();
    let app = api::build_router(Arc::clone(&state), None);
    let effect = seed_tree(&state).await;
    let display_id = seed_display_zone(&state, effect).await;
    let document = read_document(&app).await;
    let scene_id = serde_json::from_value(document["data"]["id"].clone()).expect("scene id");
    let zone_id = serde_json::from_value(primary_zone(&document)["id"].clone()).expect("zone id");
    let revision = document["data"]["revision"].as_u64().expect("revision");
    let mut output = sample_output("new-output", Some("ch1"));
    let duplicate = MemberState {
        zone_id,
        output: output.clone(),
        index: 2,
    };
    output.zone_name = Some("ch3".into());
    output.size.x = 0.0;
    let invalid_geometry = MemberState {
        zone_id,
        output: output.clone(),
        index: 2,
    };
    output.size.x = 0.2;
    let display_target = MemberState {
        zone_id: display_id,
        output,
        index: 0,
    };
    for targets in [
        vec![duplicate.clone(), duplicate],
        vec![invalid_geometry],
        vec![display_target],
    ] {
        let request = EditMembersRequest {
            assignment: None,
            scene_id,
            changes: targets
                .into_iter()
                .map(|after| MemberEdit {
                    before: None,
                    after: Some(after),
                })
                .collect(),
        };
        let rejected = send(
            &app,
            if_match(
                json_request("POST", "/api/v1/scene/members/edit".into(), json!(request)),
                revision,
            ),
        )
        .await;
        assert_eq!(rejected.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(read_document(&app).await["data"], document["data"]);
    }
}

#[tokio::test]
async fn membership_edit_preserves_multiple_attachment_instances_on_one_segment() {
    use hypercolor_types::api::scene::{
        EditMembersRequest, EditMembersResponse, MemberEdit, MemberState,
    };
    use hypercolor_types::spatial::OutputComponent;
    let (state, _tmp) = isolated_state();
    let app = api::build_router(Arc::clone(&state), None);
    seed_tree(&state).await;
    let created = send(
        &app,
        json_request(
            "POST",
            "/api/v1/scene/zones".into(),
            json!({"name": "Fans"}),
        ),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let target_id =
        serde_json::from_value(body_json(created).await["data"]["id"].clone()).expect("zone id");
    let document = read_document(&app).await;
    let scene_id = serde_json::from_value(document["data"]["id"].clone()).expect("scene id");
    let source_id = serde_json::from_value(primary_zone(&document)["id"].clone()).expect("zone id");
    // Three physical fans share one controller segment. Each attachment's
    // own output identity and LED span must survive assignment and replay.
    let changes = (0_u32..3)
        .map(|instance| {
            let mut output = sample_output(&format!("fan-{instance}"), Some("channel-2"));
            output.device_id = "nollie:controller".into();
            output.topology = LedTopology::Strip {
                count: 20,
                direction: StripDirection::LeftToRight,
            };
            output.attachment = Some(OutputComponent {
                template_id: "lian-li-sl-infinity-fan".into(),
                slot_id: "channel-2".into(),
                instance,
                led_start: Some(256 + instance * 20),
                led_count: Some(20),
                led_mapping: None,
            });
            MemberEdit {
                before: None,
                after: Some(MemberState {
                    zone_id: source_id,
                    output,
                    index: 2 + instance as usize,
                }),
            }
        })
        .collect();
    let added = send(
        &app,
        if_match(
            json_request(
                "POST",
                "/api/v1/scene/members/edit".into(),
                json!(EditMembersRequest {
                    scene_id,
                    changes,
                    assignment: None
                }),
            ),
            document["data"]["revision"].as_u64().expect("revision"),
        ),
    )
    .await;
    assert_eq!(added.status(), StatusCode::OK);
    let added: EditMembersResponse =
        serde_json::from_value(body_json(added).await["data"].clone()).expect("receipt");
    // Even an unrelated removal must not revalidate the scene using a
    // coarser device/segment key than the canonical output identity.
    let removed = send(
        &app,
        if_match(
            json_request(
                "POST",
                "/api/v1/scene/members/edit".into(),
                json!(EditMembersRequest {
                    assignment: None,
                    scene_id,
                    changes: vec![MemberEdit {
                        before: Some(MemberState {
                            zone_id: source_id,
                            output: sample_output("out-a", Some("ch1")),
                            index: 0
                        }),
                        after: None,
                    }],
                }),
            ),
            added.document.revision,
        ),
    )
    .await;
    assert_eq!(removed.status(), StatusCode::OK);
    let removed: EditMembersResponse =
        serde_json::from_value(body_json(removed).await["data"].clone()).expect("receipt");
    let moved = send(
        &app,
        if_match(
            json_request(
                "POST",
                "/api/v1/scene/members/edit".into(),
                json!(EditMembersRequest {
                    assignment: Some(hypercolor_types::api::scene::MemberAssignmentTarget {
                        zone_id: target_id,
                        device_id: "nollie:controller".into(),
                        segments: Vec::new(),
                        placements: Vec::new(),
                    }),
                    scene_id,
                    changes: Vec::new(),
                }),
            ),
            removed.document.revision,
        ),
    )
    .await;
    assert_eq!(moved.status(), StatusCode::OK);
    let moved: EditMembersResponse =
        serde_json::from_value(body_json(moved).await["data"].clone()).expect("receipt");
    for (index, change) in moved.changes.iter().enumerate() {
        let after = change.after.as_ref().expect("moved fan");
        let attachment = after
            .output
            .attachment
            .as_ref()
            .expect("canonical attachment retained");
        assert_eq!(attachment.instance as usize, index);
        assert_eq!(attachment.led_start, Some(256 + attachment.instance * 20));
        assert_eq!(after.zone_id, target_id);
    }
    let undo = EditMembersRequest {
        assignment: None,
        scene_id,
        changes: moved
            .changes
            .iter()
            .map(|change| MemberEdit {
                before: change.after.clone(),
                after: change.before.clone(),
            })
            .collect(),
    };
    let restored = send(
        &app,
        if_match(
            json_request("POST", "/api/v1/scene/members/edit".into(), json!(undo)),
            moved.document.revision,
        ),
    )
    .await;
    assert_eq!(restored.status(), StatusCode::OK);
    let restored: EditMembersResponse =
        serde_json::from_value(body_json(restored).await["data"].clone()).expect("receipt");
    for (change, previous) in restored.changes.iter().zip(&moved.changes) {
        assert_eq!(change.after, previous.before);
    }
}

#[tokio::test]
async fn membership_edit_assignment_uses_canonical_device_layout_hints() {
    use hypercolor_types::api::scene::{
        EditMembersRequest, EditMembersResponse, MemberAssignmentTarget,
    };
    use hypercolor_types::device::{
        ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceFeatures,
        DeviceId, DeviceInfo, DeviceOrigin, DeviceTopologyHint, SegmentInfo, SegmentLayoutHint,
    };
    use hypercolor_types::spatial::ZoneShape;
    let (state, _tmp) = isolated_state();
    let app = api::build_router(Arc::clone(&state), None);
    seed_tree(&state).await;
    let hint = SegmentLayoutHint::custom_grid(3, 2, &[(0, 0), (2, 1)])
        .with_size(NormalizedPosition::new(0.3, 0.2))
        .with_shape(ZoneShape::Rectangle);
    let info = DeviceInfo {
        id: DeviceId::new(),
        name: "Custom-grid microphone".into(),
        vendor: "TestVendor".into(),
        family: DeviceFamily::named("test"),
        model: None,
        connection_type: ConnectionType::Network,
        origin: DeviceOrigin::native("test", "test", ConnectionType::Network),
        segments: vec![
            SegmentInfo {
                name: "Lights".into(),
                led_count: 2,
                topology: DeviceTopologyHint::Strip,
                color_format: DeviceColorFormat::Rgb,
                layout_hint: Some(hint.clone()),
            },
            SegmentInfo {
                name: "Display".into(),
                led_count: 1,
                topology: DeviceTopologyHint::Display {
                    width: 1,
                    height: 1,
                    circular: false,
                    format: hypercolor_types::device::DisplayFrameFormat::default(),
                },
                color_format: DeviceColorFormat::Rgb,
                layout_hint: None,
            },
        ],
        firmware_version: None,
        capabilities: DeviceCapabilities {
            led_count: 2,
            supports_direct: true,
            supports_brightness: false,
            has_display: true,
            display_resolution: Some((1, 1)),
            max_fps: 60,
            color_space: hypercolor_types::device::DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        },
    };
    let device_id =
        hypercolor_core::device::DeviceLifecycleManager::canonical_layout_device_id(&info, None);
    let _ = state.device_registry.add(info).await;
    let document = read_document(&app).await;
    let scene_id = serde_json::from_value(document["data"]["id"].clone()).expect("scene id");
    let zone_id = serde_json::from_value(primary_zone(&document)["id"].clone()).expect("zone id");
    let request = EditMembersRequest {
        scene_id,
        changes: Vec::new(),
        assignment: Some(MemberAssignmentTarget {
            zone_id,
            device_id,
            segments: Vec::new(),
            placements: Vec::new(),
        }),
    };
    let assigned = send(
        &app,
        if_match(
            json_request("POST", "/api/v1/scene/members/edit".into(), json!(request)),
            document["data"]["revision"].as_u64().expect("revision"),
        ),
    )
    .await;
    assert_eq!(assigned.status(), StatusCode::OK);
    let assigned: EditMembersResponse =
        serde_json::from_value(body_json(assigned).await["data"].clone()).expect("receipt");
    assert_eq!(
        assigned.changes.len(),
        1,
        "display segments never enter LED membership"
    );
    let output = &assigned.changes[0]
        .after
        .as_ref()
        .expect("minted output")
        .output;
    assert_eq!(Some(output.topology.clone()), hint.topology);
    assert_eq!(Some(output.size), hint.size);
    assert_eq!(output.shape, hint.shape);
    assert_eq!(output.zone_name.as_deref(), Some("Lights"));
    let unchanged = send(
        &app,
        if_match(
            json_request("POST", "/api/v1/scene/members/edit".into(), json!(request)),
            assigned.document.revision,
        ),
    )
    .await;
    assert_eq!(unchanged.status(), StatusCode::OK);
    let unchanged: EditMembersResponse =
        serde_json::from_value(body_json(unchanged).await["data"].clone()).expect("receipt");
    assert!(unchanged.changes.is_empty());
    assert_eq!(unchanged.document.revision, assigned.document.revision);

    let removed = send(
        &app,
        if_match(
            json_request(
                "POST",
                "/api/v1/scene/members/edit".into(),
                json!(EditMembersRequest {
                    scene_id,
                    assignment: None,
                    changes: vec![hypercolor_types::api::scene::MemberEdit {
                        before: assigned.changes[0].after.clone(),
                        after: None,
                    }],
                }),
            ),
            assigned.document.revision,
        ),
    )
    .await;
    assert_eq!(removed.status(), StatusCode::OK);
    let removed: EditMembersResponse =
        serde_json::from_value(body_json(removed).await["data"].clone()).expect("receipt");
    let mut seeded = request;
    seeded.assignment.as_mut().expect("assignment").placements =
        vec![hypercolor_types::api::scene::MemberPlacementHint {
            segment: Some("Lights".into()),
            position: NormalizedPosition::new(0.7, 0.6),
            size: NormalizedPosition::new(0.15, 0.1),
            rotation: 0.3,
            scale: 1.2,
            orientation: None,
        }];
    let seeded = send(
        &app,
        if_match(
            json_request("POST", "/api/v1/scene/members/edit".into(), json!(seeded)),
            removed.document.revision,
        ),
    )
    .await;
    assert_eq!(seeded.status(), StatusCode::OK);
    let seeded: EditMembersResponse =
        serde_json::from_value(body_json(seeded).await["data"].clone()).expect("receipt");
    let output = &seeded.changes[0]
        .after
        .as_ref()
        .expect("seeded output")
        .output;
    assert_eq!(Some(output.topology.clone()), hint.topology);
    assert_eq!(output.shape, hint.shape);
    assert_eq!(output.position, NormalizedPosition::new(0.7, 0.6));
    assert_eq!(output.size, NormalizedPosition::new(0.15, 0.1));
}
