//! A screen on the wire (Spec 95 §6.7): the `Display` view zone each
//! connected screen is listed as, the layer routes addressed to a view, and
//! the face routes, each mapped onto the screen's layers in the zone that
//! places it.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::SystemTime;

use axum::body::Body;
use http::{Method, Request, StatusCode};
use hypercolor_core::config::ConfigManager;
use hypercolor_core::effect::EffectEntry;
use hypercolor_core::scene::screens::screen_zone_id;
use hypercolor_core::spatial::DISPLAY_ZONE_NAME;
use hypercolor_daemon::api;
use hypercolor_daemon::app_state::AppState;
use hypercolor_daemon::domain::scene::commit_scene;
use hypercolor_daemon::simulators::{SimulatedDisplayExt, activate_simulated_displays};
use hypercolor_types::api::simulators::SimulatedDisplay;
use hypercolor_types::control::ControlValue;
use hypercolor_types::device::DeviceId;
use hypercolor_types::effect::{
    ControlDefinition, ControlKind, ControlType, EffectCategory, EffectId, EffectMetadata,
    EffectSource, EffectState,
};
use hypercolor_types::layer::{BlendMode, LayerScope, LayerSource, SceneLayer, SceneLayerId};
use hypercolor_types::scene::{DisplayFaceTarget, Scene, SceneId, Zone, ZoneId, ZoneRole};
use hypercolor_types::spatial::{LedTopology, NormalizedPosition, Output, SpatialLayout};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

static DATA_DIR_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

struct Rig {
    state: Arc<AppState>,
    app: axum::Router,
    _tempdir: tempfile::TempDir,
}

fn rig() -> Rig {
    let _lock = DATA_DIR_LOCK
        .lock()
        .expect("data dir lock should not be poisoned");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let data_dir = tempdir.path().join("data");
    std::fs::create_dir_all(&data_dir).expect("temp data dir should be created");
    ConfigManager::set_data_dir_override(Some(data_dir));
    let state = Arc::new(AppState::new());
    ConfigManager::set_data_dir_override(None);
    let app = api::build_router(Arc::clone(&state), None);
    Rig {
        state,
        app,
        _tempdir: tempdir,
    }
}

/// One connected screen: its device, its view's id, and its output.
#[derive(Clone)]
struct Screen {
    device: DeviceId,
    view: String,
    output: String,
    layout_id: String,
}

impl Rig {
    /// Connect a simulated display, and optionally make it an output of
    /// the primary zone.
    async fn screen(&self, name: &str, placed: bool) -> Screen {
        let config = SimulatedDisplay {
            id: DeviceId::from_uuid(Uuid::now_v7()),
            name: name.to_owned(),
            width: 480,
            height: 480,
            circular: true,
            enabled: true,
        }
        .normalized();
        self.state
            .simulated_displays
            .write()
            .await
            .upsert(config.clone());
        activate_simulated_displays(
            &self.state.driver_host().discovery_runtime(),
            &self.state.simulated_displays,
        )
        .await
        .expect("simulated display should activate");
        self.state.domains.display.sync_connected_surfaces().await;
        let (scene_id, layout_id) = {
            let manager = self.state.scene_manager.snapshot().await;
            (
                *manager.active_scene_id().expect("a scene is active"),
                manager
                    .screen_surfaces()
                    .iter()
                    .find(|(_, surface)| surface.device_id == config.id)
                    .map(|(layout_id, _)| layout_id.clone())
                    .expect("a registered display is a known screen"),
            )
        };
        let screen = Screen {
            device: config.id,
            view: screen_zone_id(scene_id, config.id).to_string(),
            output: format!("{layout_id}-display"),
            layout_id,
        };
        if placed {
            self.place(&screen).await;
        }
        screen
    }

    /// Give the primary zone the screen's output through the member edit
    /// route, the way a client places a screen.
    async fn place(&self, screen: &Screen) {
        let document = self.ok(Method::GET, "/api/v1/scene", None).await;
        let primary = document["zones"]
            .as_array()
            .expect("zones")
            .iter()
            .find(|zone| zone["role"] == "primary")
            .expect("the scene has a primary zone");
        let request = json!({
            "scene_id": document["id"],
            "changes": [{
                "before": null,
                "after": {
                    "zone_id": primary["id"],
                    "index": primary["members"].as_array().map_or(0, Vec::len),
                    "output": Output {
                        id: screen.output.clone(),
                        name: "Screen".to_owned(),
                        device_id: screen.layout_id.clone(),
                        zone_name: Some(DISPLAY_ZONE_NAME.to_owned()),
                        position: NormalizedPosition::new(0.5, 0.5),
                        size: NormalizedPosition::new(0.2, 0.2),
                        rotation: 0.0,
                        scale: 1.0,
                        display_order: 0,
                        orientation: None,
                        topology: LedTopology::Point,
                        led_positions: Vec::new(),
                        led_mapping: None,
                        sampling_mode: None,
                        edge_behavior: None,
                        shape: None,
                        shape_preset: None,
                        attachment: None,
                        brightness: None,
                    },
                },
            }],
        });
        let request = Request::builder()
            .method(Method::POST)
            .uri("/api/v1/scene/members/edit")
            .header(http::header::CONTENT_TYPE, "application/json")
            .header(
                http::header::IF_MATCH,
                format!("\"{}\"", document["revision"]),
            )
            .body(Body::from(request.to_string()))
            .expect("request should build");
        let response = self
            .app
            .clone()
            .oneshot(request)
            .await
            .expect("request should run");
        assert!(
            response.status().is_success(),
            "placing the screen answered {}",
            response.status()
        );
    }

    /// Register a face effect with one numeric control, `speed`.
    async fn face(&self, name: &str) -> EffectId {
        self.effect(name, EffectCategory::Display).await
    }

    async fn effect(&self, name: &str, category: EffectCategory) -> EffectId {
        let metadata = EffectMetadata {
            id: EffectId::from(Uuid::now_v7()),
            name: name.to_owned(),
            author: "test".to_owned(),
            version: "0.1.0".to_owned(),
            description: format!("{name} effect"),
            category,
            tags: Vec::new(),
            controls: vec![ControlDefinition {
                id: "speed".to_owned(),
                name: "speed".to_owned(),
                kind: ControlKind::Number,
                control_type: ControlType::Slider,
                default_value: ControlValue::Float(0.5),
                min: Some(0.0),
                max: Some(1.0),
                step: None,
                labels: Vec::new(),
                group: None,
                tooltip: None,
                aspect_lock: None,
                preview_source: None,
                binding: None,
            }],
            presets: Vec::new(),
            audio_reactive: false,
            screen_reactive: false,
            input_reactive: false,
            source: EffectSource::Html {
                path: format!("/tmp/{name}.html").into(),
            },
            license: None,
        };
        let effect_id = metadata.id;
        let _ = self
            .state
            .domains
            .effects
            .register(EffectEntry {
                metadata,
                source_path: format!("/tmp/{name}.html").into(),
                modified: SystemTime::now(),
                state: EffectState::Loading,
            })
            .await;
        effect_id
    }

    async fn call(&self, method: Method, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        let request = Request::builder().method(method).uri(uri);
        let request = match body {
            Some(body) => request
                .header(http::header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string())),
            None => request.body(Body::empty()),
        }
        .expect("request should build");
        let response = self
            .app
            .clone()
            .oneshot(request)
            .await
            .expect("request should run");
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("failed to read response body");
        let value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).expect("failed to parse JSON body")
        };
        (status, value)
    }

    async fn ok(&self, method: Method, uri: &str, body: Option<Value>) -> Value {
        let (status, value) = self.call(method, uri, body).await;
        assert!(status.is_success(), "{uri} answered {status}: {value}");
        value["data"].clone()
    }

    async fn scene(&self) -> Scene {
        self.state
            .scene_manager
            .snapshot()
            .await
            .active_scene()
            .cloned()
            .expect("a scene is active")
    }

    async fn primary(&self) -> Zone {
        self.scene()
            .await
            .primary_zone()
            .cloned()
            .expect("the scene has a primary zone")
    }

    /// Rewrite the active scene directly, the way a newer client's writes
    /// would have left it.
    async fn rewrite(&self, edit: impl FnOnce(&mut Scene)) {
        let mut mutation = self.state.scene_manager.begin_mutation().await;
        let mut scene = mutation
            .scenes()
            .active_scene()
            .cloned()
            .expect("a scene is active");
        edit(&mut scene);
        mutation
            .update_scene(scene)
            .expect("the scene should update");
        commit_scene(&self.state.domains.scene, mutation)
            .await
            .expect("the rewrite should commit");
    }

    /// Put a layer in the primary zone scoped to `screens`, and return its id.
    async fn scoped_layer(&self, source: LayerSource, screens: &[&Screen]) -> String {
        let layer = SceneLayer {
            source,
            scope: Some(LayerScope {
                outputs: screens.iter().map(|screen| screen.output.clone()).collect(),
            }),
            ..SceneLayer::from_effect(
                SceneLayerId::new(),
                EffectId::from(Uuid::nil()),
                HashMap::new(),
                HashMap::new(),
                None,
            )
        };
        let id = layer.id.to_string();
        self.rewrite(|scene| {
            scene
                .primary_zone_mut()
                .expect("primary zone")
                .layers
                .push(layer.normalized());
        })
        .await;
        id
    }

    async fn view(&self, screen: &Screen) -> Value {
        self.ok(
            Method::GET,
            &format!("/api/v1/scene/zones/{}", screen.view),
            None,
        )
        .await
    }

    async fn put_face(&self, screen: &Screen, effect: EffectId, scope: &str) -> Value {
        self.ok(
            Method::PUT,
            &format!("/api/v1/displays/{}/face", screen.device),
            Some(json!({ "effect_id": effect.to_string(), "scope": scope })),
        )
        .await
    }

    async fn get_face(&self, screen: &Screen) -> Value {
        self.ok(
            Method::GET,
            &format!("/api/v1/displays/{}/face", screen.device),
            None,
        )
        .await
    }
}

fn face_layout(device: DeviceId) -> SpatialLayout {
    hypercolor_core::scene::screens::screen_face_layout(device, "Pump", 480, 480)
}

fn effect_source(effect: EffectId) -> LayerSource {
    LayerSource::Effect {
        effect_id: effect,
        controls: HashMap::new(),
        control_bindings: HashMap::new(),
        preset_id: None,
    }
}

fn fill_source() -> LayerSource {
    LayerSource::ColorFill {
        rgba: [1.0, 0.0, 0.0, 1.0],
    }
}

fn effect_body(effect: EffectId) -> Value {
    json!({ "source": { "type": "effect", "effect_id": effect.to_string(), "controls": {} } })
}

fn fill_body() -> Value {
    json!({ "source": { "type": "color_fill", "rgba": [0.0, 0.0, 1.0, 1.0] } })
}

fn layer_ids(zone: &Value) -> Vec<String> {
    zone["layers"]
        .as_array()
        .expect("a zone lists its layers")
        .iter()
        .map(|layer| layer["id"].as_str().expect("layer id").to_owned())
        .collect()
}

/// The ids and scopes of the primary zone's scoped layers, bottom to top.
fn scoped(zone: &Zone) -> Vec<(String, Vec<String>)> {
    zone.layers
        .iter()
        .filter_map(|layer| {
            layer
                .scope
                .as_ref()
                .map(|scope| (layer.id.to_string(), scope.outputs.clone()))
        })
        .collect()
}

fn sorted(mut ids: Vec<String>) -> Vec<String> {
    ids.sort();
    ids
}

// ── The view zone ────────────────────────────────────────────────────────

#[tokio::test]
async fn the_scene_lists_one_display_zone_per_connected_screen() {
    let rig = rig();
    let face = rig.face("clock").await;
    let faced = rig.screen("Pump", true).await;
    let bare = rig.screen("Fan", false).await;
    rig.ok(
        Method::PUT,
        &format!("/api/v1/displays/{}/face", faced.device),
        Some(json!({
            "effect_id": face.to_string(),
            "scope": "scene",
            "blend_mode": "screen",
            "opacity": 0.4
        })),
    )
    .await;
    let primary = rig.primary().await;

    let document = rig.ok(Method::GET, "/api/v1/scene", None).await;
    let zones = document["zones"].as_array().expect("zones");
    let displays = zones
        .iter()
        .filter(|zone| zone["role"] == "display")
        .collect::<Vec<_>>();
    assert_eq!(displays.len(), 2, "one per connected screen, faced or not");

    let view = displays
        .iter()
        .find(|zone| zone["id"] == faced.view)
        .expect("the faced screen's view has the derived id");
    assert_eq!(view["name"], "Pump");
    assert_eq!(view["enabled"], true);
    assert_eq!(
        view["display_target"]["device_id"],
        faced.device.to_string()
    );
    assert_eq!(view["display_target"]["blend_mode"], "screen");
    assert!((view["display_target"]["opacity"].as_f64().expect("opacity") - 0.4).abs() < 1e-6);
    assert_eq!(view["layers"].as_array().map(Vec::len), Some(1));
    assert!(
        view["layers"][0].get("scope").is_none(),
        "a view lists its layers with the scope omitted"
    );
    assert_eq!(view["layers"][0]["source"]["effect_id"], face.to_string());
    assert_eq!(view["members"].as_array().map(Vec::len), Some(0));
    assert!(view.get("layout").is_none_or(Value::is_null));
    assert_eq!(view["view_of"]["zone"], primary.id.to_string());
    assert_eq!(view["view_of"]["member"], faced.output);

    let empty = displays
        .iter()
        .find(|zone| zone["id"] == bare.view)
        .expect("a screen with no layers stays listed");
    assert_eq!(empty["name"], "Fan");
    assert_eq!(empty["enabled"], true);
    assert_eq!(empty["layers"].as_array().map(Vec::len), Some(0));
    assert_eq!(
        empty["display_target"]["device_id"],
        bare.device.to_string()
    );
    assert!(
        empty["display_target"].get("blend_mode").is_none()
            && empty["display_target"].get("opacity").is_none(),
        "an empty stack reads as a fresh target: alpha at full opacity"
    );
    assert!(
        empty.get("view_of").is_none(),
        "no zone places the bare screen"
    );
}

#[tokio::test]
async fn a_stored_display_zone_is_listed_under_its_own_id() {
    let rig = rig();
    let face = rig.face("clock").await;
    let screen = rig.screen("Pump", true).await;
    // A first layer that is not opaque keeps the zone from converting.
    let mut layer = SceneLayer::from_effect(
        SceneLayerId::new(),
        face,
        HashMap::new(),
        HashMap::new(),
        None,
    );
    layer.opacity = 0.5;
    let stored = Zone {
        id: ZoneId::new(),
        name: "Pump Face".to_owned(),
        description: None,
        layers: vec![layer],
        layout: face_layout(screen.device),
        brightness: 1.0,
        enabled: false,
        color: None,
        display_target: Some(DisplayFaceTarget {
            device_id: screen.device,
            blend_mode: BlendMode::Add,
            opacity: 0.5,
        }),
        role: ZoneRole::Display,
        controls_version: 0,
        layers_version: 0,
    };
    let stored_id = stored.id.to_string();
    rig.rewrite(|scene| scene.zones.push(stored)).await;

    let document = rig.ok(Method::GET, "/api/v1/scene", None).await;
    let displays = document["zones"]
        .as_array()
        .expect("zones")
        .iter()
        .filter(|zone| zone["role"] == "display")
        .collect::<Vec<_>>();
    assert_eq!(displays.len(), 1, "the stored zone stands for the screen");
    assert_eq!(displays[0]["id"], stored_id);
    assert_eq!(displays[0]["enabled"], false, "its own flag");
    assert_eq!(displays[0]["display_target"]["blend_mode"], "add");
    assert!(displays[0].get("view_of").is_none());

    let (status, _) = rig
        .call(
            Method::GET,
            &format!("/api/v1/scene/zones/{}", screen.view),
            None,
        )
        .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "the derived id addresses nothing while a stored zone exists"
    );
}

#[tokio::test]
async fn a_view_written_back_in_a_scene_replace_is_skipped() {
    let rig = rig();
    let face = rig.face("clock").await;
    let created = rig
        .ok(
            Method::POST,
            "/api/v1/scenes",
            Some(json!({ "name": "Desk" })),
        )
        .await;
    let scene_id = created["id"].as_str().expect("scene id").to_owned();
    rig.ok(
        Method::POST,
        &format!("/api/v1/scenes/{scene_id}/activate"),
        Some(json!({})),
    )
    .await;
    let screen = rig.screen("Pump", true).await;
    rig.put_face(&screen, face, "scene").await;
    // A screen no zone places keeps its face in a stored display zone that
    // carries the view's id, and that zone is content, not an echo.
    let unplaced = rig.screen("Fan", false).await;
    rig.put_face(&unplaced, face, "scene").await;
    let stored_before = rig.scene().await;

    // A client that edits the live document and saves it as the scene
    // sends the screen's view along with the zones.
    let live = rig.ok(Method::GET, "/api/v1/scene", None).await;
    assert!(
        live["zones"]
            .as_array()
            .expect("zones")
            .iter()
            .any(|zone| zone["role"] == "display" && zone.get("view_of").is_some()),
        "the live document lists the view"
    );
    let mut document = rig
        .ok(Method::GET, &format!("/api/v1/scenes/{scene_id}"), None)
        .await;
    assert_eq!(
        document["zones"]
            .as_array()
            .expect("zones")
            .iter()
            .filter(|zone| zone["role"] == "display")
            .count(),
        1,
        "a stored scene lists its display zone and no views"
    );
    document["zones"] = live["zones"].clone();

    rig.ok(
        Method::PUT,
        &format!("/api/v1/scenes/{scene_id}"),
        Some(document),
    )
    .await;

    let stored = rig.scene().await;
    assert_eq!(
        stored
            .zones
            .iter()
            .filter(|zone| zone.role == ZoneRole::Display)
            .map(|zone| zone.id.to_string())
            .collect::<Vec<_>>(),
        vec![unplaced.view.clone()],
        "the view did not become a display zone, and the stored one was kept"
    );
    assert_eq!(
        stored
            .display_zone_for(unplaced.device)
            .map(|zone| &zone.layers),
        stored_before
            .display_zone_for(unplaced.device)
            .map(|zone| &zone.layers)
    );
    assert_eq!(
        scoped(stored.primary_zone().expect("primary zone")),
        scoped(stored_before.primary_zone().expect("primary zone")),
        "the screen's face is still one layer of the zone that places it"
    );
}

#[tokio::test]
async fn a_view_is_refused_where_a_display_zone_always_was() {
    let rig = rig();
    let face = rig.face("clock").await;
    let screen = rig.screen("Pump", true).await;
    rig.put_face(&screen, face, "scene").await;
    let zone = format!("/api/v1/scene/zones/{}", screen.view);

    for (method, uri, body) in [
        (
            Method::POST,
            "/api/v1/scene/clear".to_owned(),
            Some(json!({ "zone": screen.view })),
        ),
        (Method::PATCH, zone.clone(), Some(json!({ "name": "Mine" }))),
        (Method::DELETE, zone, None),
    ] {
        let (status, body) = rig.call(method.clone(), &uri, body).await;
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{method} {uri} answered {body}"
        );
        assert_eq!(body["error"]["code"], "validation_error");
    }
    assert_eq!(layer_ids(&rig.view(&screen).await).len(), 1);

    // Clearing the scene's lighting leaves the screen's face, as it left
    // a display zone's.
    rig.ok(Method::POST, "/api/v1/scene/clear", Some(json!({})))
        .await;
    assert_eq!(layer_ids(&rig.view(&screen).await).len(), 1);
    assert_eq!(
        rig.get_face(&screen).await["effect"]["id"],
        face.to_string()
    );
}

#[tokio::test]
async fn placing_a_screen_converts_its_stored_face_in_the_same_commit() {
    let rig = rig();
    let face = rig.face("clock").await;
    let screen = rig.screen("Pump", false).await;
    let written = rig
        .ok(
            Method::PUT,
            &format!("/api/v1/displays/{}/face", screen.device),
            Some(json!({
                "effect_id": face.to_string(),
                "scope": "scene",
                "blend_mode": "screen",
                "opacity": 0.5
            })),
        )
        .await;
    let stored_layers = layer_ids(&written["zone"]);
    assert!(rig.scene().await.display_zone_for(screen.device).is_some());
    let revision = rig.state.scene_manager.revision();

    rig.place(&screen).await;

    assert_eq!(
        rig.state.scene_manager.revision(),
        revision + 1,
        "the placement and the conversion are one commit"
    );
    let scene = rig.scene().await;
    assert!(
        scene.display_zone_for(screen.device).is_none(),
        "no stored zone is left to cover a layer written for the screen"
    );
    let primary = scene.primary_zone().expect("primary zone");
    assert_eq!(
        scoped(primary),
        vec![(stored_layers[0].clone(), vec![screen.output.clone()])]
    );
    let layer = primary.layers.last().expect("the face layer");
    assert_eq!(layer.blend, BlendMode::Screen, "the target moved onto the layer");
    assert!((layer.opacity - 0.5).abs() < f32::EPSILON);

    // The zone id the client held goes on addressing the screen.
    let view = rig.view(&screen).await;
    assert_eq!(view["id"], screen.view);
    assert_eq!(layer_ids(&view), stored_layers);
    assert_eq!(view["view_of"]["member"], screen.output);
    let read = rig.get_face(&screen).await;
    assert_face(&read, face, &screen.view, "scene", (true, false), "scene");
}

// ── The zone's own effect ────────────────────────────────────────────────

#[tokio::test]
async fn the_active_effect_is_never_a_face_or_a_scoped_overlay() {
    let rig = rig();
    let aurora = rig.effect("aurora", EffectCategory::Ambient).await;
    let ripple = rig.effect("ripple", EffectCategory::Ambient).await;
    let face = rig.face("clock").await;
    let screen = rig.screen("Pump", true).await;
    let active = || async {
        rig.state
            .domains
            .effects
            .active_primary_effect()
            .await
            .map(|(_, effect)| effect.id)
    };
    let layer = |effect: EffectId, scope: Option<&str>| {
        let mut layer = SceneLayer::from_effect(
            SceneLayerId::new(),
            effect,
            HashMap::new(),
            HashMap::new(),
            None,
        );
        layer.scope = scope.map(|output| LayerScope {
            outputs: vec![output.to_owned()],
        });
        layer
    };

    // A screen's face on top of the lighting.
    let stack = vec![layer(aurora, None), layer(face, Some(&screen.output))];
    rig.rewrite(|scene| scene.primary_zone_mut().expect("primary").layers = stack)
        .await;
    assert_eq!(active().await, Some(aurora));

    // An overlay scoped to one output on top of the lighting.
    let stack = vec![layer(aurora, None), layer(ripple, Some(&screen.output))];
    rig.rewrite(|scene| scene.primary_zone_mut().expect("primary").layers = stack)
        .await;
    assert_eq!(active().await, Some(aurora));

    // Only scoped layers: the zone runs no effect of its own, which is what
    // clearing the lighting under a face leaves behind.
    let stack = vec![layer(face, Some(&screen.output))];
    rig.rewrite(|scene| scene.primary_zone_mut().expect("primary").layers = stack)
        .await;
    assert_eq!(active().await, None);
}

// ── Layer routes addressed to a view ─────────────────────────────────────

#[tokio::test]
async fn listing_a_views_layers_answers_the_screens_stack_bottom_to_top() {
    let rig = rig();
    let face = rig.face("clock").await;
    let screen = rig.screen("Pump", true).await;
    let other = rig.screen("Fan", true).await;
    let first = rig.scoped_layer(effect_source(face), &[&screen]).await;
    rig.scoped_layer(fill_source(), &[&other]).await;
    let second = rig.scoped_layer(fill_source(), &[&screen, &other]).await;

    let listed = rig
        .ok(
            Method::GET,
            &format!("/api/v1/scene/zones/{}/layers", screen.view),
            None,
        )
        .await;

    assert_eq!(listed["total"], 2);
    let items = listed["items"].as_array().expect("items");
    assert_eq!(items[0]["id"], first);
    assert_eq!(items[1]["id"], second);
    assert!(items.iter().all(|layer| layer.get("scope").is_none()));
}

#[tokio::test]
async fn creating_a_layer_on_a_view_scopes_it_to_the_screen() {
    let rig = rig();
    let face = rig.face("clock").await;
    let screen = rig.screen("Pump", true).await;

    let (status, created) = rig
        .call(
            Method::POST,
            &format!("/api/v1/scene/zones/{}/layers", screen.view),
            Some(effect_body(face)),
        )
        .await;

    assert_eq!(status, StatusCode::CREATED);
    let view = &created["data"];
    assert_eq!(view["id"], screen.view, "the answer is the view");
    assert_eq!(view["role"], "display");
    let ids = layer_ids(view);
    assert_eq!(ids.len(), 1);
    let primary = rig.primary().await;
    assert_eq!(view["view_of"]["zone"], primary.id.to_string());
    assert_eq!(
        scoped(&primary),
        vec![(ids[0].clone(), vec![screen.output.clone()])]
    );
    assert_eq!(
        primary.layers.last().map(|layer| layer.id.to_string()),
        Some(ids[0].clone()),
        "the layer lands at the top of the owning stack"
    );

    let (status, refused) = rig
        .call(
            Method::POST,
            &format!("/api/v1/scene/zones/{}/layers", screen.view),
            Some(json!({
                "source": { "type": "color_fill", "rgba": [0.0, 0.0, 1.0, 1.0] },
                "scope": { "outputs": [screen.output] }
            })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(refused["error"]["code"], "validation_error");
    assert_eq!(layer_ids(&rig.view(&screen).await).len(), 1);
}

#[tokio::test]
async fn creating_a_layer_for_a_screen_no_zone_places_stores_a_display_zone() {
    let rig = rig();
    let face = rig.face("clock").await;
    let screen = rig.screen("Pump", false).await;
    let zones_before = rig.scene().await.zones;

    let (status, created) = rig
        .call(
            Method::POST,
            &format!("/api/v1/scene/zones/{}/layers", screen.view),
            Some(effect_body(face)),
        )
        .await;

    assert_eq!(status, StatusCode::CREATED);
    let zone = &created["data"];
    assert_eq!(
        zone["id"], screen.view,
        "the stored zone answers to the id the client addressed"
    );
    assert_eq!(zone["role"], "display");
    assert!(zone.get("view_of").is_none(), "no zone places the screen");
    assert_eq!(layer_ids(zone).len(), 1);
    let scene = rig.scene().await;
    let stored = scene
        .display_zone_for(screen.device)
        .expect("the layer is stored in a display zone");
    assert_eq!(stored.id.to_string(), screen.view);
    assert_eq!(stored.layers.len(), 1);
    assert_eq!(
        scene
            .zones
            .iter()
            .filter(|zone| zone.role != ZoneRole::Display)
            .cloned()
            .collect::<Vec<_>>(),
        zones_before,
        "no zone gains an output or a layer on the screen's behalf"
    );
    assert_eq!(
        rig.get_face(&screen).await["effect"]["id"],
        face.to_string()
    );
    assert_eq!(rig.get_face(&screen).await["rendering"], "scene");
}

#[tokio::test]
async fn replacing_a_sole_layer_keeps_it_on_the_screen_whatever_the_request_says() {
    let rig = rig();
    let face = rig.face("clock").await;
    let screen = rig.screen("Pump", true).await;
    let other = rig.screen("Fan", true).await;
    let below = rig.scoped_layer(fill_source(), &[&other]).await;
    let sole = rig.scoped_layer(fill_source(), &[&screen]).await;
    let above = rig.scoped_layer(fill_source(), &[&other]).await;

    let mut current = sole;
    for scope in [
        None,
        Some(Value::Null),
        Some(json!({ "outputs": [other.output] })),
    ] {
        let mut body = effect_body(face);
        if let Some(scope) = scope {
            body["scope"] = scope;
        }
        let view = rig
            .ok(
                Method::PUT,
                &format!("/api/v1/scene/zones/{}/layers/{current}", screen.view),
                Some(body),
            )
            .await;

        assert_eq!(view["id"], screen.view);
        let ids = layer_ids(&view);
        assert_eq!(ids.len(), 1);
        assert_eq!(view["layers"][0]["source"]["effect_id"], face.to_string());
        current.clone_from(&ids[0]);
        assert_eq!(
            scoped(&rig.primary().await),
            vec![
                (below.clone(), vec![other.output.clone()]),
                (current.clone(), vec![screen.output.clone()]),
                (above.clone(), vec![other.output.clone()]),
            ],
            "replaced in place, still on this screen alone"
        );
    }
}

#[tokio::test]
async fn replacing_a_shared_layer_splits_it_and_leaves_the_other_screen_alone() {
    let rig = rig();
    let face = rig.face("clock").await;
    let screen = rig.screen("Pump", true).await;
    let other = rig.screen("Fan", true).await;
    let shared = rig.scoped_layer(fill_source(), &[&screen, &other]).await;
    let other_before = rig.view(&other).await;

    let view = rig
        .ok(
            Method::PUT,
            &format!("/api/v1/scene/zones/{}/layers/{shared}", screen.view),
            Some(effect_body(face)),
        )
        .await;

    let ids = layer_ids(&view);
    assert_eq!(ids.len(), 1);
    assert_ne!(ids[0], shared, "this screen's copy has an id of its own");
    assert_eq!(view["layers"][0]["source"]["effect_id"], face.to_string());
    assert_eq!(
        scoped(&rig.primary().await),
        vec![
            (shared.clone(), vec![other.output.clone()]),
            (ids[0].clone(), vec![screen.output.clone()]),
        ],
        "the replacement sits directly above the layer it split from"
    );
    assert_eq!(
        rig.view(&other).await["layers"],
        other_before["layers"],
        "the other screen keeps the original"
    );
}

#[tokio::test]
async fn deleting_a_layer_removes_a_sole_one_and_narrows_a_shared_one() {
    let rig = rig();
    let screen = rig.screen("Pump", true).await;
    let other = rig.screen("Fan", true).await;
    let sole = rig.scoped_layer(fill_source(), &[&screen]).await;
    let shared = rig.scoped_layer(fill_source(), &[&screen, &other]).await;
    let other_before = rig.view(&other).await;

    let view = rig
        .ok(
            Method::DELETE,
            &format!("/api/v1/scene/zones/{}/layers/{sole}", screen.view),
            None,
        )
        .await;
    assert_eq!(view["id"], screen.view);
    assert_eq!(layer_ids(&view), vec![shared.clone()]);

    let view = rig
        .ok(
            Method::DELETE,
            &format!("/api/v1/scene/zones/{}/layers/{shared}", screen.view),
            None,
        )
        .await;
    assert!(
        layer_ids(&view).is_empty(),
        "the view is empty and still listed"
    );
    assert_eq!(
        scoped(&rig.primary().await),
        vec![(shared.clone(), vec![other.output.clone()])]
    );
    assert_eq!(rig.view(&other).await["layers"], other_before["layers"]);

    let (status, _) = rig
        .call(
            Method::DELETE,
            &format!("/api/v1/scene/zones/{}/layers/{shared}", screen.view),
            None,
        )
        .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a layer that is no longer this screen's is not found"
    );
}

#[tokio::test]
async fn reordering_a_view_moves_only_its_layers_and_splits_shared_ones() {
    let rig = rig();
    let screen = rig.screen("Pump", true).await;
    let other = rig.screen("Fan", true).await;
    let theirs_low = rig.scoped_layer(fill_source(), &[&other]).await;
    let shared = rig.scoped_layer(fill_source(), &[&screen, &other]).await;
    let theirs_high = rig.scoped_layer(fill_source(), &[&other]).await;
    let sole = rig.scoped_layer(fill_source(), &[&screen]).await;
    let other_before = layer_ids(&rig.view(&other).await);
    let uri = format!("/api/v1/scene/zones/{}/layers/order", screen.view);

    let same = rig
        .ok(
            Method::PATCH,
            &uri,
            Some(json!({ "order": [shared, sole] })),
        )
        .await;
    assert_eq!(layer_ids(&same), vec![shared.clone(), sole.clone()]);
    assert_eq!(
        scoped(&rig.primary().await).len(),
        4,
        "an order that changes nothing splits nothing"
    );

    let (status, _) = rig
        .call(Method::PATCH, &uri, Some(json!({ "order": [sole] })))
        .await;
    assert!(
        status.is_client_error(),
        "an order must name every layer of the view once"
    );

    let view = rig
        .ok(
            Method::PATCH,
            &uri,
            Some(json!({ "order": [sole, shared] })),
        )
        .await;

    assert_eq!(view["id"], screen.view);
    let ids = layer_ids(&view);
    assert_eq!(ids.len(), 2);
    assert_eq!(ids[0], sole);
    assert_ne!(ids[1], shared, "the shared layer was split first");
    let stack = scoped(&rig.primary().await);
    assert_eq!(
        stack,
        vec![
            (theirs_low, vec![other.output.clone()]),
            (shared, vec![other.output.clone()]),
            (sole, vec![screen.output.clone()]),
            (theirs_high, vec![other.output.clone()]),
            (ids[1].clone(), vec![screen.output.clone()]),
        ],
        "this screen's layers trade the positions they held; no other layer moves"
    );
    assert_eq!(
        layer_ids(&rig.view(&other).await),
        other_before,
        "the other screen keeps its order"
    );
}

#[tokio::test]
async fn patching_controls_through_a_view_patches_this_screens_layer_alone() {
    let rig = rig();
    let face = rig.face("clock").await;
    let screen = rig.screen("Pump", true).await;
    let other = rig.screen("Fan", true).await;
    let sole = rig.scoped_layer(effect_source(face), &[&screen]).await;
    let shared = rig
        .scoped_layer(effect_source(face), &[&screen, &other])
        .await;
    let patch = json!({ "values": { "speed": { "kind": "float", "value": 0.75 } } });
    let speed_of = |zone: &Value, id: &str| {
        zone["layers"]
            .as_array()
            .expect("layers")
            .iter()
            .find(|layer| layer["id"] == id)
            .map(|layer| layer["source"]["controls"]["speed"]["value"].clone())
    };

    let patched = rig
        .ok(
            Method::PATCH,
            &format!("/api/v1/scene/zones/{}/layers/{sole}/controls", screen.view),
            Some(patch.clone()),
        )
        .await;
    assert_eq!(patched["id"], screen.view);
    assert_eq!(speed_of(&patched, &sole), Some(json!(0.75)));
    assert_eq!(
        scoped(&rig.primary().await).len(),
        2,
        "a sole layer is patched in place"
    );

    let patched = rig
        .ok(
            Method::PATCH,
            &format!(
                "/api/v1/scene/zones/{}/layers/{shared}/controls",
                screen.view
            ),
            Some(patch),
        )
        .await;
    let ids = layer_ids(&patched);
    assert_eq!(ids.len(), 2);
    assert_ne!(ids[1], shared, "the shared layer was split first");
    assert_eq!(speed_of(&patched, &ids[1]), Some(json!(0.75)));
    let theirs = rig.view(&other).await;
    assert_eq!(layer_ids(&theirs), vec![shared.clone()]);
    assert_ne!(
        speed_of(&theirs, &shared),
        Some(json!(0.75)),
        "the other screen's controls are untouched"
    );
}

// ── The unchanged Studio path ────────────────────────────────────────────

#[tokio::test]
async fn a_client_that_only_knows_display_zones_can_add_remove_and_replace_a_face() {
    let rig = rig();
    let clock = rig.face("clock").await;
    let weather = rig.face("weather").await;
    let screen = rig.screen("Pump", true).await;
    let layers = format!("/api/v1/scene/zones/{}/layers", screen.view);

    // The first face on a screen that has no layers.
    let document = rig.ok(Method::GET, "/api/v1/scene", None).await;
    let listed = document["zones"]
        .as_array()
        .expect("zones")
        .iter()
        .find(|zone| zone["role"] == "display")
        .cloned()
        .expect("the empty screen is listed, or its first face has nowhere to go");
    assert_eq!(listed["id"], screen.view);
    let view = rig
        .ok(Method::POST, &layers, Some(effect_body(clock)))
        .await;
    let first = layer_ids(&view).remove(0);
    assert_eq!(
        rig.get_face(&screen).await["effect"]["id"],
        clock.to_string()
    );

    // Delete the screen's last layer, then add one again.
    let view = rig
        .ok(Method::DELETE, &format!("{layers}/{first}"), None)
        .await;
    assert!(layer_ids(&view).is_empty());
    assert!(rig.get_face(&screen).await.is_null());
    assert_eq!(
        rig.view(&screen).await["id"],
        screen.view,
        "the emptied screen keeps its view"
    );
    let view = rig
        .ok(Method::POST, &layers, Some(effect_body(clock)))
        .await;
    let second = layer_ids(&view).remove(0);

    // Replace the face with a request that carries no scope.
    let view = rig
        .ok(
            Method::PUT,
            &format!("{layers}/{second}"),
            Some(effect_body(weather)),
        )
        .await;
    let replaced = layer_ids(&view).remove(0);
    assert_eq!(
        view["layers"][0]["source"]["effect_id"],
        weather.to_string()
    );
    assert_eq!(
        scoped(&rig.primary().await),
        vec![(replaced, vec![screen.output.clone()])],
        "it stays on that screen"
    );
    assert_eq!(
        rig.get_face(&screen).await["effect"]["id"],
        weather.to_string()
    );
}

// ── The face routes ──────────────────────────────────────────────────────

/// Assert the fields `GET /displays/{id}/face` answers with.
fn assert_face(
    payload: &Value,
    effect: EffectId,
    zone: &str,
    live_scope: &str,
    assigned: (bool, bool),
    rendering: &str,
) {
    assert_eq!(payload["effect"]["id"], effect.to_string(), "effect");
    assert_eq!(payload["zone"]["id"], zone, "zone");
    assert_eq!(payload["live_scope"], live_scope, "live_scope");
    assert_eq!(payload["scene_assigned"], assigned.0, "scene_assigned");
    assert_eq!(payload["default_assigned"], assigned.1, "default_assigned");
    assert_eq!(payload["rendering"], rendering, "rendering");
}

#[tokio::test]
async fn reading_a_face_answers_row_by_row() {
    let rig = rig();
    let clock = rig.face("clock").await;
    let kept = rig.face("kept").await;
    let screen = rig.screen("Pump", true).await;
    let layers = format!("/api/v1/scene/zones/{}/layers", screen.view);

    // No screen layers, no kept face.
    assert!(rig.get_face(&screen).await.is_null());

    // Other layers only, enabled, no kept face.
    let fill = layer_ids(&rig.ok(Method::POST, &layers, Some(fill_body())).await).remove(0);
    assert!(rig.get_face(&screen).await.is_null());

    // Other layers only, enabled, kept face stored: the changed row.
    let stored = rig.put_face(&screen, kept, "default").await;
    let kept_zone = stored["zone"]["id"].as_str().expect("kept zone").to_owned();
    assert_face(
        &rig.get_face(&screen).await,
        kept,
        &kept_zone,
        "default",
        (false, true),
        "scene",
    );

    // Other layers only, disabled, kept face stored.
    let mut disabled = fill_body();
    disabled["enabled"] = json!(false);
    let fill = layer_ids(
        &rig.ok(Method::PUT, &format!("{layers}/{fill}"), Some(disabled))
            .await,
    )
    .remove(0);
    assert_face(
        &rig.get_face(&screen).await,
        kept,
        &kept_zone,
        "default",
        (false, true),
        "default",
    );

    // No screen layers, kept face stored.
    rig.ok(Method::DELETE, &format!("{layers}/{fill}"), None)
        .await;
    assert_face(
        &rig.get_face(&screen).await,
        kept,
        &kept_zone,
        "default",
        (false, true),
        "default",
    );

    // Face layer, enabled.
    let face = layer_ids(
        &rig.ok(Method::POST, &layers, Some(effect_body(clock)))
            .await,
    )
    .remove(0);
    assert_face(
        &rig.get_face(&screen).await,
        clock,
        &screen.view,
        "scene",
        (true, true),
        "scene",
    );

    // Face layer in a paused zone.
    rig.rewrite(|scene| scene.primary_zone_mut().expect("primary").enabled = false)
        .await;
    assert_face(
        &rig.get_face(&screen).await,
        clock,
        &screen.view,
        "scene",
        (true, true),
        "none",
    );
    rig.rewrite(|scene| scene.primary_zone_mut().expect("primary").enabled = true)
        .await;

    // Face layer, disabled.
    let mut disabled = effect_body(clock);
    disabled["enabled"] = json!(false);
    rig.ok(Method::PUT, &format!("{layers}/{face}"), Some(disabled))
        .await;
    assert_face(
        &rig.get_face(&screen).await,
        clock,
        &screen.view,
        "scene",
        (true, true),
        "none",
    );
}

#[tokio::test]
async fn reading_a_face_whose_effect_is_not_installed_is_not_found() {
    let rig = rig();
    let screen = rig.screen("Pump", true).await;
    rig.scoped_layer(effect_source(EffectId::from(Uuid::now_v7())), &[&screen])
        .await;

    let (status, body) = rig
        .call(
            Method::GET,
            &format!("/api/v1/displays/{}/face", screen.device),
            None,
        )
        .await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "effect_not_found");
}

#[tokio::test]
async fn putting_a_scene_face_makes_it_the_screens_whole_stack() {
    let rig = rig();
    let clock = rig.face("clock").await;
    let screen = rig.screen("Pump", true).await;
    let other = rig.screen("Fan", true).await;
    let uri = format!("/api/v1/displays/{}/face", screen.device);

    let written = rig.put_face(&screen, clock, "scene").await;
    assert_face(
        &written,
        clock,
        &screen.view,
        "scene",
        (true, false),
        "scene",
    );
    assert_eq!(written["scene_id"], SceneId::DEFAULT.to_string());
    assert_eq!(written["device_id"], screen.device.to_string());
    let primary = rig.primary().await;
    let stack = scoped(&primary);
    assert_eq!(stack.len(), 1);
    assert_eq!(stack[0].1, vec![screen.output.clone()]);
    let layer = primary.layers.last().expect("the face layer");
    assert_eq!(
        layer.blend,
        BlendMode::Alpha,
        "an omitted blend means alpha"
    );
    assert!((layer.opacity - 1.0).abs() < f32::EPSILON);
    assert!(layer.enabled);

    // Sole layers are removed; the screen leaves shared ones.
    let shared = rig.scoped_layer(fill_source(), &[&screen, &other]).await;
    let written = rig
        .ok(
            Method::PUT,
            &uri,
            Some(json!({
                "effect_id": clock.to_string(),
                "scope": "scene",
                "blend_mode": "replace",
                "opacity": 0.3,
                "controls": { "speed": { "kind": "float", "value": 0.25 } }
            })),
        )
        .await;
    assert_face(
        &written,
        clock,
        &screen.view,
        "scene",
        (true, false),
        "scene",
    );
    let ids = layer_ids(&written["zone"]);
    assert_eq!(ids.len(), 1, "the stack becomes one layer");
    assert_eq!(
        written["zone"]["layers"][0]["source"]["controls"]["speed"]["value"],
        json!(0.25)
    );
    assert_eq!(written["zone"]["display_target"]["blend_mode"], "replace");
    assert!(
        written["zone"]["display_target"].get("opacity").is_none(),
        "a replacing face is forced to full opacity"
    );
    assert_eq!(
        sorted(
            scoped(&rig.primary().await)
                .into_iter()
                .map(|(id, outputs)| format!("{id}:{}", outputs.join(",")))
                .collect()
        ),
        sorted(vec![
            format!("{shared}:{}", other.output),
            format!("{}:{}", ids[0], screen.output),
        ])
    );

    // The requests the route has always refused.
    let (status, _) = rig
        .call(
            Method::PUT,
            &uri,
            Some(json!({ "effect_id": Uuid::now_v7().to_string(), "scope": "scene" })),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let lighting = rig.effect("aurora", EffectCategory::Ambient).await;
    let (status, body) = rig
        .call(
            Method::PUT,
            &uri,
            Some(json!({ "effect_id": lighting.to_string(), "scope": "scene" })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"], "validation_error");
    let (status, _) = rig
        .call(
            Method::PUT,
            &format!("/api/v1/displays/{}/face", DeviceId::new()),
            Some(json!({ "effect_id": clock.to_string(), "scope": "scene" })),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "an unknown device");
}

#[tokio::test]
async fn putting_a_scene_face_for_a_screen_no_zone_places_stores_a_display_zone() {
    let rig = rig();
    let clock = rig.face("clock").await;
    let screen = rig.screen("Pump", false).await;
    let zones_before = rig.scene().await.zones;

    let written = rig
        .ok(
            Method::PUT,
            &format!("/api/v1/displays/{}/face", screen.device),
            Some(json!({
                "effect_id": clock.to_string(),
                "scope": "scene",
                "blend_mode": "screen",
                "opacity": 0.5
            })),
        )
        .await;

    assert_face(
        &written,
        clock,
        &screen.view,
        "scene",
        (true, false),
        "scene",
    );
    assert_eq!(written["zone"]["display_target"]["blend_mode"], "screen");
    assert!(written["zone"].get("view_of").is_none());
    let scene = rig.scene().await;
    let stored = scene
        .display_zone_for(screen.device)
        .expect("the face is stored as a display zone");
    assert_eq!(stored.id.to_string(), screen.view);
    assert_eq!(
        scene
            .zones
            .iter()
            .filter(|zone| zone.role != ZoneRole::Display)
            .cloned()
            .collect::<Vec<_>>(),
        zones_before,
        "no zone gains an output or a layer on the screen's behalf"
    );

    let deleted = rig
        .ok(
            Method::DELETE,
            &format!("/api/v1/displays/{}/face?scope=scene", screen.device),
            None,
        )
        .await;
    assert_eq!(deleted["deleted"], true);
    assert!(rig.scene().await.display_zone_for(screen.device).is_none());
    assert!(layer_ids(&rig.view(&screen).await).is_empty());
}

#[tokio::test]
async fn putting_a_kept_face_leaves_the_scene_alone() {
    let rig = rig();
    let clock = rig.face("clock").await;
    let kept = rig.face("kept").await;
    let screen = rig.screen("Pump", true).await;
    let before = rig.scene().await;

    // An omitted scope means the kept face.
    let written = rig
        .ok(
            Method::PUT,
            &format!("/api/v1/displays/{}/face", screen.device),
            Some(json!({ "effect_id": kept.to_string() })),
        )
        .await;
    let kept_zone = written["zone"]["id"].as_str().expect("zone").to_owned();
    assert_ne!(
        kept_zone, screen.view,
        "the kept face has a zone of its own"
    );
    assert_face(
        &written,
        kept,
        &kept_zone,
        "default",
        (false, true),
        "default",
    );
    assert_eq!(rig.scene().await.zones, before.zones);

    // Under a scene face the kept one is stored and stays hidden.
    rig.put_face(&screen, clock, "scene").await;
    let written = rig.put_face(&screen, kept, "default").await;
    assert_face(&written, kept, &kept_zone, "scene", (true, true), "scene");
}

#[tokio::test]
async fn deleting_a_face_clears_the_scene_stack_or_the_kept_face() {
    let rig = rig();
    let clock = rig.face("clock").await;
    let kept = rig.face("kept").await;
    let screen = rig.screen("Pump", true).await;
    let other = rig.screen("Fan", true).await;
    let uri = format!("/api/v1/displays/{}/face", screen.device);
    rig.put_face(&screen, kept, "default").await;
    rig.put_face(&screen, clock, "scene").await;
    let shared = rig.scoped_layer(fill_source(), &[&screen, &other]).await;

    let deleted = rig
        .ok(Method::DELETE, &format!("{uri}?scope=scene"), None)
        .await;
    assert_eq!(deleted["deleted"], true);
    assert_eq!(deleted["scope"], "scene");
    assert_eq!(deleted["scene_id"], SceneId::DEFAULT.to_string());
    assert_eq!(deleted["device_id"], screen.device.to_string());
    assert_eq!(
        scoped(&rig.primary().await),
        vec![(shared, vec![other.output.clone()])],
        "sole layers go, and the screen leaves the shared one"
    );
    assert_eq!(rig.get_face(&screen).await["live_scope"], "default");

    // The scope defaults to the kept face.
    let deleted = rig.ok(Method::DELETE, &uri, None).await;
    assert_eq!(deleted["deleted"], true);
    assert_eq!(deleted["scope"], "default");
    assert!(deleted.get("scene_id").is_none_or(Value::is_null));
    assert!(rig.get_face(&screen).await.is_null());
    let deleted = rig.ok(Method::DELETE, &uri, None).await;
    assert_eq!(deleted["deleted"], false, "nothing was stored any more");
}

#[tokio::test]
async fn deleting_a_scene_face_removes_a_stored_display_zone() {
    let rig = rig();
    let face = rig.face("clock").await;
    let screen = rig.screen("Pump", true).await;
    let mut layer = SceneLayer::from_effect(
        SceneLayerId::new(),
        face,
        HashMap::new(),
        HashMap::new(),
        None,
    );
    layer.opacity = 0.5;
    let layout = face_layout(screen.device);
    rig.rewrite(|scene| {
        scene.zones.push(Zone {
            id: ZoneId::new(),
            name: "Pump Face".to_owned(),
            description: None,
            layers: vec![layer],
            layout,
            brightness: 1.0,
            enabled: true,
            color: None,
            display_target: Some(DisplayFaceTarget::new(screen.device)),
            role: ZoneRole::Display,
            controls_version: 0,
            layers_version: 0,
        });
    })
    .await;

    let deleted = rig
        .ok(
            Method::DELETE,
            &format!("/api/v1/displays/{}/face?scope=scene", screen.device),
            None,
        )
        .await;

    assert_eq!(deleted["deleted"], true);
    assert_eq!(deleted["scope"], "scene");
    assert!(rig.scene().await.display_zone_for(screen.device).is_none());
    assert!(layer_ids(&rig.view(&screen).await).is_empty());
}

#[tokio::test]
async fn patching_composition_answers_row_by_row() {
    let rig = rig();
    let clock = rig.face("clock").await;
    let kept = rig.face("kept").await;
    let screen = rig.screen("Pump", true).await;
    let other = rig.screen("Fan", true).await;
    let uri = format!("/api/v1/displays/{}/face/composition", screen.device);
    let patch = json!({ "blend_mode": "screen", "opacity": 0.6 });

    // An empty body is a validation error.
    let (status, _) = rig.call(Method::PATCH, &uri, Some(json!({}))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    // Neither assigned nor kept.
    let (status, _) = rig.call(Method::PATCH, &uri, Some(patch.clone())).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Not assigned, kept face stored: the kept face is patched.
    rig.put_face(&screen, kept, "default").await;
    let patched = rig.ok(Method::PATCH, &uri, Some(patch.clone())).await;
    assert_eq!(patched["effect"]["id"], kept.to_string());
    assert_eq!(patched["live_scope"], "default");
    assert_eq!(patched["zone"]["display_target"]["blend_mode"], "screen");
    assert!(scoped(&rig.primary().await).is_empty());

    // Assigned: the bottom enabled screen layer is patched, split if shared.
    let shared = rig
        .scoped_layer(effect_source(clock), &[&screen, &other])
        .await;
    let patched = rig.ok(Method::PATCH, &uri, Some(patch)).await;
    assert_eq!(patched["effect"]["id"], clock.to_string());
    assert_eq!(patched["zone"]["id"], screen.view);
    assert_eq!(patched["live_scope"], "scene");
    assert_eq!(patched["scene_assigned"], true);
    assert_eq!(patched["rendering"], "scene");
    assert_eq!(patched["zone"]["display_target"]["blend_mode"], "screen");
    let ids = layer_ids(&patched["zone"]);
    assert_ne!(ids[0], shared, "the shared layer was split first");
    let primary = rig.primary().await;
    let mine = primary
        .layers
        .iter()
        .find(|layer| layer.id.to_string() == ids[0])
        .expect("this screen's copy");
    assert_eq!(mine.blend, BlendMode::Screen);
    assert!((mine.opacity - 0.6).abs() < 1e-6);
    let theirs = primary
        .layers
        .iter()
        .find(|layer| layer.id.to_string() == shared)
        .expect("the original");
    assert_ne!(
        theirs.blend,
        BlendMode::Screen,
        "the other screen is untouched"
    );
}

#[tokio::test]
async fn patching_face_controls_answers_row_by_row() {
    let rig = rig();
    let clock = rig.face("clock").await;
    let kept = rig.face("kept").await;
    let screen = rig.screen("Pump", true).await;
    let other = rig.screen("Fan", true).await;
    let uri = format!("/api/v1/displays/{}/face/controls", screen.device);
    let patch = json!({ "values": { "speed": { "kind": "float", "value": 0.75 } } });

    // The requests the route has always refused.
    let (status, _) = rig
        .call(Method::PATCH, &uri, Some(json!({ "values": {} })))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _) = rig
        .call(
            Method::PATCH,
            &uri,
            Some(json!({
                "values": { "speed": { "kind": "float", "value": 0.75 } },
                "clear_bindings": ["speed"]
            })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    // Neither assigned nor kept.
    let (status, _) = rig.call(Method::PATCH, &uri, Some(patch.clone())).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Not assigned, kept face stored: merged into the kept face.
    rig.put_face(&screen, kept, "default").await;
    let patched = rig.ok(Method::PATCH, &uri, Some(patch.clone())).await;
    assert_eq!(patched["effect"]["id"], kept.to_string());
    assert_eq!(patched["live_scope"], "default");
    assert_eq!(
        patched["zone"]["layers"][0]["source"]["controls"]["speed"]["value"],
        json!(0.75)
    );

    // Assigned, exactly one face layer: patched, split if shared.
    let shared = rig
        .scoped_layer(effect_source(clock), &[&screen, &other])
        .await;
    let patched = rig.ok(Method::PATCH, &uri, Some(patch.clone())).await;
    assert_eq!(patched["effect"]["id"], clock.to_string());
    assert_eq!(patched["zone"]["id"], screen.view);
    assert_eq!(patched["live_scope"], "scene");
    assert_eq!(patched["rendering"], "scene");
    let ids = layer_ids(&patched["zone"]);
    assert_ne!(ids[0], shared, "the shared layer was split first");
    assert_eq!(
        patched["zone"]["layers"][0]["source"]["controls"]["speed"]["value"],
        json!(0.75)
    );
    assert!(
        rig.view(&other).await["layers"][0]["source"]["controls"]
            .get("speed")
            .is_none_or(|speed| speed["value"] != json!(0.75)),
        "the other screen's controls are untouched"
    );

    // Assigned, more than one face layer.
    rig.scoped_layer(effect_source(clock), &[&screen]).await;
    let (status, _) = rig.call(Method::PATCH, &uri, Some(patch)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_daemon_advertises_screen_layers() {
    let rig = rig();

    let system = rig.ok(Method::GET, "/api/v1/system", None).await;

    let capabilities = system["status"]["capabilities"]
        .as_array()
        .expect("the server lists its capabilities");
    assert!(capabilities.contains(&json!("layer-scope")));
    assert!(capabilities.contains(&json!("screen-layers")));
}
