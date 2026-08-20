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
use hypercolor_daemon::api::{self, AppState};
use hypercolor_types::api::output::OutputPowerMode;
use hypercolor_types::effect::{
    ControlBinding, ControlDefinition, ControlKind, ControlType, ControlValue, EffectCategory,
    EffectId, EffectMetadata, EffectSource, EffectState, PresetTemplate,
};
use hypercolor_types::event::{
    ChangeTrigger, EffectStopReason, EventControlValue, HypercolorEvent, LayerStackChangeKind,
    SceneSettingsChangeKind,
};
use hypercolor_types::library::PresetId;
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
        spaces: None,
        version: 1,
    }
}

/// Seed the live tree with a primary zone running one effect over two
/// member segments, which is the shape every test below reads back.
async fn seed_tree(state: &Arc<AppState>) -> EffectId {
    let metadata = sample_effect("Aurora");
    let effect_id = metadata.id;
    {
        let mut registry = state.effect_registry.write().await;
        registry.register(EffectEntry {
            metadata: metadata.clone(),
            source_path: "/tmp/aurora.rs".into(),
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
            sample_layout(vec![
                sample_output("out-a", Some("ch1")),
                sample_output("out-b", Some("ch2")),
            ]),
        )
        .expect("primary zone should seed");
    effect_id
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
    state.effect_registry.write().await.register(EffectEntry {
        metadata,
        source_path: "/tmp/first-light.rs".into(),
        modified: SystemTime::now(),
        state: EffectState::Loading,
    });
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

    let manager = state.scene_manager.read().await;
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
    hypercolor_daemon::domain::output::set_power(&state, OutputPowerMode::Paused).await;

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
        state.power_state.borrow().manually_paused(),
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
            json!({ "values": { "speed": { "float": 0.5 } } }),
        ),
    )
    .await;
    assert_eq!(
        stale.status(),
        StatusCode::NOT_FOUND,
        "a patch addressing a vanished layer 404s (Spec 78 §1.4)"
    );
    let body = body_json(stale).await;
    assert_eq!(body["error"]["code"], "not_found");
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
                json!({ "values": { "speed": { "float": 0.5 } } }),
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
                    json!({ "values": { "speed": { "float": value } } }),
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

    hypercolor_daemon::domain::output::set_power(&state, OutputPowerMode::Paused).await;

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
            state.power_state.borrow().manually_paused(),
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
        .read()
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
                "controls": { "speed": { "float": 0.7 } },
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
    assert_eq!(
        body["data"]["zone"]["layers"][0]["source"]["controls"]["speed"],
        json!({ "float": 0.7 })
    );

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
        let manager = state.scene_manager.read().await;
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
            json!({ "values": { "speed": { "float": 0.75 } } }),
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
    assert_eq!(control_event.2, EventControlValue::Number(0.25));
    assert_eq!(control_event.3, EventControlValue::Number(0.75));
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
        let mut manager = state.scene_manager.write().await;
        let zone_uuid = zone_id.parse::<Uuid>().expect("zone uuid");
        manager
            .set_group_control_binding(
                hypercolor_types::scene::ZoneId(zone_uuid),
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
            )
            .expect("binding should attach");
    }

    let refused = send(
        &app,
        json_request(
            "PATCH",
            format!("/api/v1/scene/zones/{zone_id}/layers/{layer_id}/controls"),
            json!({ "values": { "speed": { "float": 0.5 } } }),
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
            json!({ "values": { "speed": { "float": 0.5 } }, "clear_bindings": ["speed"] }),
        ),
    )
    .await;
    assert_eq!(recovered.status(), StatusCode::OK);
    let zone = body_json(recovered).await;
    assert_eq!(
        zone["data"]["layers"][0]["source"]["controls"]["speed"],
        json!({ "float": 0.5 })
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
        assert_eq!(body["error"]["code"], "not_found");
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
        let metadata = {
            let registry = state.effect_registry.read().await;
            registry
                .get(&effect_id)
                .map(|entry| entry.metadata.clone())
                .expect("seeded effect")
        };
        let mut manager = state.scene_manager.write().await;
        manager
            .upsert_display_group(
                hypercolor_types::device::DeviceId::new(),
                "Panel",
                &metadata,
                HashMap::<String, ControlValue>::new(),
                sample_layout(vec![sample_output("out-face", None)]),
            )
            .expect("face assigns")
            .id
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
async fn generic_live_tree_mutations_cannot_edit_display_owned_zones() {
    let (state, _tmp) = isolated_state();
    let app = api::build_router(Arc::clone(&state), None);
    let effect_id = seed_tree(&state).await;
    let display_zone_id = {
        let metadata = {
            let registry = state.effect_registry.read().await;
            registry
                .get(&effect_id)
                .map(|entry| entry.metadata.clone())
                .expect("seeded effect")
        };
        let mut manager = state.scene_manager.write().await;
        manager
            .upsert_display_group(
                hypercolor_types::device::DeviceId::new(),
                "Panel",
                &metadata,
                HashMap::new(),
                sample_layout(vec![sample_output("out-face", None)]),
            )
            .expect("face assigns")
            .id
    };
    let before = read_document(&app).await;
    let face = before["data"]["zones"]
        .as_array()
        .expect("zones")
        .iter()
        .find(|zone| zone["id"] == display_zone_id.to_string())
        .expect("display zone");
    let layer_id = face["layers"][0]["id"]
        .as_str()
        .expect("face layer")
        .to_owned();
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
        json_request(
            "POST",
            format!("/api/v1/scene/zones/{zone}/layers"),
            json!({
                "source": { "type": "effect", "effect_id": effect_id, "controls": {} }
            }),
        ),
        json_request(
            "PATCH",
            format!("/api/v1/scene/zones/{zone}/layers/order"),
            json!({ "order": [layer_id.clone()] }),
        ),
        json_request(
            "PUT",
            format!("/api/v1/scene/zones/{zone}/layers/{layer_id}"),
            json!({
                "source": { "type": "effect", "effect_id": effect_id, "controls": {} }
            }),
        ),
        empty_request(
            "DELETE",
            format!("/api/v1/scene/zones/{zone}/layers/{layer_id}"),
        ),
        json_request(
            "PATCH",
            format!("/api/v1/scene/zones/{zone}/layers/{layer_id}/controls"),
            json!({ "values": { "speed": { "float": 0.75 } } }),
        ),
    ];

    for request in requests {
        let response = send(&app, request).await;
        assert_eq!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "display face state belongs exclusively to the display API"
        );
    }

    let after = read_document(&app).await;
    assert_eq!(after["data"], before["data"]);
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
            json!({ "values": { "speed": { "float": 0.75 } } }),
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
