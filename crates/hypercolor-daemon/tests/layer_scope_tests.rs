//! Layer scope through the daemon's services: what the layer and member
//! routes keep, carry, and refuse, and what apply and clear leave alone.
//!
//! No request type carries a scope yet, so these tests seed scoped layers
//! through the scene mutation and then drive the public surfaces.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

use axum::body::Body;
use http::{Request, StatusCode};
use hypercolor_core::effect::EffectEntry;
use hypercolor_core::scene::LayerMutationError;
use hypercolor_daemon::api;
use hypercolor_daemon::app_state::{AppState, AppStateBuilder};
use hypercolor_daemon::domain::scene::commit_scene;
use hypercolor_daemon::domain::scene_tree::{self, LayerScopeEdit, ReplaceLayer};
use hypercolor_daemon::domain::{DomainError, DomainErrorDetails};
use hypercolor_daemon::mcp::tools::execute_tool_with_state;
use hypercolor_types::api::scene::{
    EditMembersRequest, EditMembersResponse, LayerMove, LayerSlot, MemberEdit, MemberState,
};
use hypercolor_types::api::scenes::ReplaceSceneRequest;
use hypercolor_types::config::EffectErrorFallbackPolicy;
use hypercolor_types::control::ControlValue;
use hypercolor_types::effect::{
    EffectCategory, EffectId, EffectMetadata, EffectSource, EffectState,
};
use hypercolor_types::event::{ChangeTrigger, HypercolorEvent, LayerStackChangeKind};
use hypercolor_types::layer::{BlendMode, LayerScope, SceneLayer, SceneLayerId};
use hypercolor_types::scene::{Scene, SceneId, Zone, ZoneId};
use hypercolor_types::spatial::{
    EdgeBehavior, LedTopology, NormalizedPosition, Output, SamplingMode, SpatialLayout,
    StripDirection,
};
use serde_json::json;
use tower::ServiceExt;
use uuid::Uuid;

const KEYS: &str = "out-keys";
const LOGO: &str = "out-logo";
const STRIP: &str = "out-strip";
const SCREEN: &str = "out-screen";

fn isolated_state() -> (Arc<AppState>, tempfile::TempDir) {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let data_dir = tempdir.path().join("data");
    std::fs::create_dir_all(&data_dir).expect("temp data dir should be created");
    (Arc::new(AppStateBuilder::new(data_dir).build()), tempdir)
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

fn json_request(method: &str, uri: String, body: &serde_json::Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .expect("request should build")
}

fn if_match(mut request: Request<Body>, revision: u64) -> Request<Body> {
    request.headers_mut().insert(
        http::header::IF_MATCH,
        http::HeaderValue::from_str(&format!("\"{revision}\"")).expect("valid etag"),
    );
    request
}

fn effect(name: &str) -> EffectMetadata {
    EffectMetadata {
        id: EffectId::new(Uuid::now_v7()),
        name: name.to_owned(),
        author: "test".to_owned(),
        version: "0.1.0".to_owned(),
        description: format!("{name} description"),
        category: EffectCategory::Ambient,
        tags: Vec::new(),
        controls: Vec::new(),
        presets: Vec::new(),
        audio_reactive: false,
        screen_reactive: false,
        input_reactive: false,
        source: EffectSource::Native {
            path: format!("builtin/{name}").into(),
        },
        license: None,
    }
}

async fn register(state: &Arc<AppState>, name: &str) -> EffectMetadata {
    let metadata = effect(name);
    let _ = state
        .domains
        .effects
        .register(EffectEntry {
            metadata: metadata.clone(),
            source_path: format!("/tmp/{name}.rs").into(),
            modified: SystemTime::now(),
            state: EffectState::Loading,
        })
        .await;
    metadata
}

fn output(id: &str) -> Output {
    let segment = match id {
        KEYS => "Keys",
        LOGO => "Logo",
        SCREEN => "Display",
        _ => "Channel 1",
    };
    Output {
        id: id.to_owned(),
        name: id.to_owned(),
        device_id: if id == STRIP || id == SCREEN {
            format!("mock:{id}")
        } else {
            "mock:keyboard".to_owned()
        },
        zone_name: Some(segment.to_owned()),
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

fn layout() -> SpatialLayout {
    SpatialLayout {
        id: "layout-test".to_owned(),
        name: "Layout".to_owned(),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones: [KEYS, LOGO, STRIP, SCREEN].map(output).to_vec(),
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        version: 1,
    }
}

fn scope(outputs: &[&str]) -> LayerScope {
    LayerScope {
        outputs: outputs.iter().map(|id| (*id).to_owned()).collect(),
    }
    .normalized()
}

fn scoped_layer(effect: &EffectMetadata, outputs: &[&str]) -> SceneLayer {
    let mut layer = SceneLayer::from_effect(
        SceneLayerId::new(),
        effect.id,
        HashMap::new(),
        HashMap::new(),
        None,
    );
    layer.blend = BlendMode::Screen;
    layer.scope = Some(scope(outputs));
    layer
}

/// The default scene with a primary zone holding four outputs (a
/// two-output keyboard, a strip, and a screen) under one base effect, and
/// an empty custom zone.
struct Rig {
    state: Arc<AppState>,
    app: axum::Router,
    _tmp: tempfile::TempDir,
    bubbles: EffectMetadata,
    ripple: EffectMetadata,
    main: ZoneId,
    desk: ZoneId,
}

async fn rig() -> Rig {
    let (state, tmp) = isolated_state();
    let bubbles = register(&state, "Bubbles").await;
    let ripple = register(&state, "Ripple").await;
    let mut mutation = state.scene_manager.begin_mutation().await;
    let main = mutation
        .upsert_primary_zone(
            &bubbles,
            HashMap::<String, ControlValue>::new(),
            None,
            layout(),
            ChangeTrigger::System,
            None,
        )
        .expect("primary zone")
        .id;
    let desk = mutation
        .create_zone(SceneId::DEFAULT, "Desk".to_owned(), None, (320, 200))
        .expect("custom zone");
    commit_scene(&state.domains.scene, mutation)
        .await
        .expect("seed commit");
    let app = api::build_router(Arc::clone(&state), None);
    Rig {
        state,
        app,
        _tmp: tmp,
        bubbles,
        ripple,
        main,
        desk,
    }
}

impl Rig {
    /// Seed a scoped layer on top of a zone's stack.
    async fn add(&self, zone_id: ZoneId, outputs: &[&str]) -> SceneLayerId {
        let layer = scoped_layer(&self.ripple, outputs);
        let layer_id = layer.id;
        let mut mutation = self.state.scene_manager.begin_mutation().await;
        mutation
            .insert_layer(SceneId::DEFAULT, zone_id, layer, None, None)
            .expect("scoped layer");
        commit_scene(&self.state.domains.scene, mutation)
            .await
            .expect("layer commit");
        layer_id
    }

    async fn scene(&self) -> Scene {
        self.state
            .scene_manager
            .snapshot()
            .await
            .active_scene()
            .cloned()
            .expect("active scene")
    }

    async fn zone(&self, zone_id: ZoneId) -> Zone {
        self.scene()
            .await
            .zones
            .into_iter()
            .find(|zone| zone.id == zone_id)
            .expect("zone")
    }

    async fn stack(&self, zone_id: ZoneId) -> Vec<SceneLayerId> {
        self.zone(zone_id)
            .await
            .layers
            .iter()
            .map(|layer| layer.id)
            .collect()
    }

    fn revision(&self) -> u64 {
        self.state.scene_manager.revision()
    }

    async fn member(&self, zone_id: ZoneId, id: &str) -> MemberState {
        let zone = self.zone(zone_id).await;
        let index = zone
            .layout
            .zones
            .iter()
            .position(|output| output.id == id)
            .expect("member");
        MemberState {
            zone_id,
            output: zone.layout.zones[index].clone(),
            index,
        }
    }

    async fn edit(&self, request: &EditMembersRequest) -> axum::response::Response {
        send(
            &self.app,
            if_match(
                json_request("POST", "/api/v1/scene/members/edit".into(), &json!(request)),
                self.revision(),
            ),
        )
        .await
    }

    /// Move outputs from the primary zone to the custom zone in one edit.
    async fn move_to_desk(&self, outputs: &[&str]) -> EditMembersResponse {
        let mut changes = Vec::new();
        for (index, id) in outputs.iter().enumerate() {
            let before = self.member(self.main, id).await;
            let mut after = before.clone();
            after.zone_id = self.desk;
            after.index = index;
            changes.push(MemberEdit {
                before: Some(before),
                after: Some(after),
            });
        }
        let response = self
            .edit(&EditMembersRequest {
                scene_id: SceneId::DEFAULT,
                changes,
                assignment: None,
                layer_moves: None,
            })
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        serde_json::from_value(body_json(response).await["data"].clone()).expect("receipt")
    }
}

fn reversed(receipt: &EditMembersResponse) -> EditMembersRequest {
    EditMembersRequest {
        scene_id: SceneId::DEFAULT,
        changes: receipt
            .changes
            .iter()
            .map(|change| MemberEdit {
                before: change.after.clone(),
                after: change.before.clone(),
            })
            .collect(),
        assignment: None,
        layer_moves: Some(
            receipt
                .layer_moves
                .iter()
                .map(LayerMove::reversed)
                .collect(),
        ),
    }
}

// ── Replace ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_layer_replace_that_says_nothing_about_scope_keeps_it() {
    let rig = rig().await;
    let layer_id = rig.add(rig.main, &[KEYS, LOGO]).await;

    let response = send(
        &rig.app,
        json_request(
            "PUT",
            format!("/api/v1/scene/zones/{}/layers/{layer_id}", rig.main),
            &json!({
                "source": {"type": "effect", "effect_id": rig.ripple.id, "controls": {}},
                "blend": "add",
            }),
        ),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let zone = body_json(response).await;
    let replaced = &zone["data"]["layers"][1];
    assert_ne!(replaced["id"], json!(layer_id), "a replace mints a new id");
    assert_eq!(replaced["blend"], "add");
    assert_eq!(
        replaced["scope"],
        json!({"outputs": [KEYS, LOGO]}),
        "a client that does not know scope must not widen the layer"
    );
}

#[tokio::test]
async fn a_layer_replace_sets_clears_and_validates_scope_through_the_domain() {
    let rig = rig().await;
    let layer_id = rig.add(rig.main, &[KEYS]).await;
    let replace = |layer_id, scope| {
        let mut layer = scoped_layer(&rig.ripple, &[]);
        layer.scope = None;
        scene_tree::replace_layer(
            &rig.state.domains.scene_tree,
            ReplaceLayer {
                zone_id: rig.main,
                layer_id,
                layer,
                scope,
                fence: None,
            },
        )
    };

    let refused = replace(
        layer_id,
        LayerScopeEdit::Set(scope(&[KEYS, "out-elsewhere"])),
    )
    .await
    .expect_err("a new scope id must be a member");
    match refused {
        DomainError::Validation {
            details: Some(DomainErrorDetails::UnknownMember { unknown_member }),
            ..
        } => assert_eq!(unknown_member, "out-elsewhere"),
        other => panic!("expected a validation error naming the output, got {other:?}"),
    }
    assert_eq!(rig.stack(rig.main).await[1], layer_id, "nothing changed");

    let widened = replace(layer_id, LayerScopeEdit::Set(scope(&[KEYS, STRIP])))
        .await
        .expect("both ids are members");
    let layer = &widened.zone.layers[1];
    assert_eq!(layer.scope, Some(scope(&[KEYS, STRIP])));

    let cleared = replace(layer.id, LayerScopeEdit::Clear)
        .await
        .expect("clearing a scope introduces nothing");
    assert_eq!(cleared.zone.layers[1].scope, None);
}

#[tokio::test]
async fn creating_a_scoped_layer_checks_membership_at_commit() {
    let rig = rig().await;

    let refused = hypercolor_daemon::domain::layer::insert_layer(
        &rig.state.domains.effects,
        rig.main,
        scoped_layer(&rig.ripple, &["out-elsewhere"]),
        None,
        None,
    )
    .await
    .expect("the scene accepts the mutation attempt");
    assert!(matches!(
        refused,
        Err(LayerMutationError::ScopeOutputMissing { output, .. }) if output == "out-elsewhere"
    ));

    let created = hypercolor_daemon::domain::layer::insert_layer(
        &rig.state.domains.effects,
        rig.main,
        scoped_layer(&rig.ripple, &[STRIP]),
        None,
        None,
    )
    .await
    .expect("the scene accepts the mutation attempt")
    .expect("the strip is a member");
    assert_eq!(created.zone.layers[1].scope, Some(scope(&[STRIP])));
}

// ── Membership edits carry layers ────────────────────────────────────────

#[tokio::test]
async fn a_membership_edit_carries_a_scoped_layer_and_reports_it() {
    let rig = rig().await;
    let keyboard = rig.add(rig.main, &[KEYS, LOGO]).await;
    let mut events = rig.state.event_bus.subscribe_all();

    let receipt = rig.move_to_desk(&[KEYS, LOGO]).await;

    assert_eq!(
        receipt.layer_moves,
        vec![LayerMove {
            layer_id: keyboard,
            from: LayerSlot {
                zone_id: rig.main,
                index: 1,
            },
            to: LayerSlot {
                zone_id: rig.desk,
                index: 0,
            },
            scope_before: scope(&[KEYS, LOGO]),
            scope_after: scope(&[KEYS, LOGO]),
        }]
    );
    let desk = receipt
        .document
        .zones
        .iter()
        .find(|zone| zone.id == rig.desk)
        .expect("desk");
    assert_eq!(desk.layers.len(), 1);
    assert_eq!(desk.layers[0].id, keyboard);
    assert_eq!(desk.layers[0].scope, Some(scope(&[KEYS, LOGO])));
    assert_eq!(rig.stack(rig.main).await.len(), 1, "only the base remains");

    let mut stack_changes = Vec::new();
    while let Ok(event) = events.try_recv() {
        if let HypercolorEvent::LayerStackChanged { zone_id, kind, .. } = event.event {
            stack_changes.push((zone_id, kind));
        }
    }
    assert!(
        stack_changes.contains(&(rig.main, LayerStackChangeKind::Removed)),
        "{stack_changes:?}"
    );
    assert!(
        stack_changes.contains(&(rig.desk, LayerStackChangeKind::Created)),
        "{stack_changes:?}"
    );
}

#[tokio::test]
async fn a_carried_layer_undoes_exactly_through_the_receipt() {
    let rig = rig().await;
    let keyboard = rig.add(rig.main, &[KEYS, LOGO]).await;
    let strip = rig.add(rig.main, &[STRIP]).await;
    let narrowed = rig.add(rig.main, &[KEYS, STRIP]).await;
    let before = rig.scene().await;

    let receipt = rig.move_to_desk(&[KEYS, LOGO]).await;
    assert_eq!(receipt.layer_moves.len(), 2);
    assert_eq!(rig.stack(rig.desk).await, vec![keyboard]);
    assert_eq!(
        rig.zone(rig.main).await.layers[2].scope,
        Some(scope(&[STRIP])),
        "the layer that kept one output lost the other from its scope"
    );

    let undone = rig.edit(&reversed(&receipt)).await;
    assert_eq!(undone.status(), StatusCode::OK);
    let undone: EditMembersResponse =
        serde_json::from_value(body_json(undone).await["data"].clone()).expect("receipt");

    let after = rig.scene().await;
    // An edit regenerates the LED positions of the outputs it places, so
    // membership is compared by identity and placement.
    let placed = |zone: &Zone| {
        zone.layout
            .zones
            .iter()
            .map(|output| (output.id.clone(), output.position, output.size))
            .collect::<Vec<_>>()
    };
    for (restored, original) in after.zones.iter().zip(&before.zones) {
        assert_eq!(restored.layers, original.layers, "{}", original.name);
        assert_eq!(placed(restored), placed(original), "{}", original.name);
    }
    let base = before.zones[0].layers[0].id;
    assert_eq!(
        rig.stack(rig.main).await,
        vec![base, keyboard, strip, narrowed],
        "every layer is back at its slot"
    );

    // The undo's own receipt redoes the edit.
    let redone = rig.edit(&reversed(&undone)).await;
    assert_eq!(redone.status(), StatusCode::OK);
    assert_eq!(rig.stack(rig.desk).await, vec![keyboard]);
    assert_eq!(rig.stack(rig.main).await, vec![base, strip, narrowed]);
}

#[tokio::test]
async fn an_empty_replayed_move_list_carries_nothing() {
    let rig = rig().await;
    let keyboard = rig.add(rig.main, &[KEYS, LOGO]).await;
    let keys = rig.member(rig.main, KEYS).await;
    let logo = rig.member(rig.main, LOGO).await;
    let to_desk = |before: MemberState, index| {
        let mut after = before.clone();
        after.zone_id = rig.desk;
        after.index = index;
        MemberEdit {
            before: Some(before),
            after: Some(after),
        }
    };

    let response = rig
        .edit(&EditMembersRequest {
            scene_id: SceneId::DEFAULT,
            changes: vec![to_desk(keys, 0), to_desk(logo, 1)],
            assignment: None,
            layer_moves: Some(Vec::new()),
        })
        .await;

    assert_eq!(response.status(), StatusCode::OK);
    let main = rig.zone(rig.main).await;
    assert_eq!(
        main.layers[1].id, keyboard,
        "a replay moves only what it names"
    );
    assert!(main.layer_is_dormant(&main.layers[1]));
}

#[tokio::test]
async fn a_replayed_layer_move_that_no_longer_fits_is_a_conflict() {
    let rig = rig().await;
    rig.add(rig.main, &[KEYS, LOGO]).await;
    let receipt = rig.move_to_desk(&[KEYS, LOGO]).await;
    let mut stale = reversed(&receipt);
    stale
        .layer_moves
        .as_mut()
        .expect("layer moves")
        .first_mut()
        .expect("one move")
        .from
        .index = 3;
    let landed = rig.scene().await;

    let response = rig.edit(&stale).await;

    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        rig.scene().await,
        landed,
        "a refused replay changes nothing"
    );
}

#[tokio::test]
async fn a_replayed_layer_move_cannot_widen_a_scope_to_a_non_member() {
    let rig = rig().await;
    rig.add(rig.main, &[KEYS, LOGO]).await;
    let receipt = rig.move_to_desk(&[KEYS, LOGO]).await;
    let mut widened = reversed(&receipt);
    widened.layer_moves.as_mut().expect("layer moves")[0].scope_after =
        scope(&[KEYS, LOGO, "out-elsewhere"]);

    let response = rig.edit(&widened).await;

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = body_json(response).await;
    assert_eq!(body["error"]["details"]["unknown_member"], "out-elsewhere");
}

#[tokio::test]
async fn a_replay_with_an_unsorted_scope_returns_a_receipt_that_undoes_it() {
    let rig = rig().await;
    let keyboard = rig.add(rig.main, &[KEYS, LOGO]).await;
    let before = rig.scene().await;
    let mut changes = Vec::new();
    for (index, id) in [KEYS, LOGO].into_iter().enumerate() {
        let from = rig.member(rig.main, id).await;
        let mut to = from.clone();
        to.zone_id = rig.desk;
        to.index = index;
        changes.push(MemberEdit {
            before: Some(from),
            after: Some(to),
        });
    }
    let unsorted = |outputs: &[&str]| LayerScope {
        outputs: outputs.iter().map(|id| (*id).to_owned()).collect(),
    };

    let replayed = rig
        .edit(&EditMembersRequest {
            scene_id: SceneId::DEFAULT,
            changes,
            assignment: None,
            layer_moves: Some(vec![LayerMove {
                layer_id: keyboard,
                from: LayerSlot {
                    zone_id: rig.main,
                    index: 1,
                },
                to: LayerSlot {
                    zone_id: rig.desk,
                    index: 0,
                },
                scope_before: unsorted(&[LOGO, KEYS]),
                scope_after: unsorted(&[LOGO, KEYS, KEYS]),
            }]),
        })
        .await;

    assert_eq!(replayed.status(), StatusCode::OK);
    let receipt: EditMembersResponse =
        serde_json::from_value(body_json(replayed).await["data"].clone()).expect("receipt");
    assert_eq!(
        (
            &receipt.layer_moves[0].scope_before,
            &receipt.layer_moves[0].scope_after
        ),
        (&scope(&[KEYS, LOGO]), &scope(&[KEYS, LOGO])),
        "the receipt names the scopes as they were committed"
    );
    assert_eq!(
        rig.zone(rig.desk).await.layers[0].scope,
        Some(receipt.layer_moves[0].scope_after.clone())
    );

    let undone = rig.edit(&reversed(&receipt)).await;
    assert_eq!(
        undone.status(),
        StatusCode::OK,
        "a receipt from any response reverses cleanly"
    );
    let after = rig.scene().await;
    for (restored, original) in after.zones.iter().zip(&before.zones) {
        assert_eq!(restored.layers, original.layers, "{}", original.name);
    }
}

#[tokio::test]
async fn layer_moves_are_refused_beside_an_assignment_target() {
    let rig = rig().await;

    let response = rig
        .edit(&EditMembersRequest {
            scene_id: SceneId::DEFAULT,
            changes: Vec::new(),
            assignment: Some(hypercolor_types::api::scene::MemberAssignmentTarget {
                zone_id: rig.desk,
                device_id: "mock:keyboard".to_owned(),
                segments: Vec::new(),
                placements: Vec::new(),
            }),
            layer_moves: Some(Vec::new()),
        })
        .await;

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn an_assignment_target_carries_a_devices_layer_with_it() {
    let rig = rig().await;
    let keyboard = rig.add(rig.main, &[KEYS, LOGO]).await;

    let response = rig
        .edit(&EditMembersRequest {
            scene_id: SceneId::DEFAULT,
            changes: Vec::new(),
            assignment: Some(hypercolor_types::api::scene::MemberAssignmentTarget {
                zone_id: rig.desk,
                device_id: "mock:keyboard".to_owned(),
                segments: Vec::new(),
                placements: Vec::new(),
            }),
            layer_moves: None,
        })
        .await;

    assert_eq!(response.status(), StatusCode::OK);
    let receipt: EditMembersResponse =
        serde_json::from_value(body_json(response).await["data"].clone()).expect("receipt");
    assert_eq!(receipt.changes.len(), 2, "both keyboard outputs moved");
    assert_eq!(receipt.layer_moves.len(), 1);
    assert_eq!(rig.stack(rig.desk).await, vec![keyboard]);
}

#[tokio::test]
async fn assigning_a_devices_segments_to_a_zone_carries_their_layer() {
    let rig = rig().await;
    let keyboard = rig.add(rig.main, &[KEYS, LOGO]).await;

    let response = send(
        &rig.app,
        json_request(
            "POST",
            format!("/api/v1/scene/zones/{}/members", rig.desk),
            &json!({"device_id": "mock:keyboard", "segments": ["Keys", "Logo"]}),
        ),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        rig.stack(rig.desk).await,
        vec![keyboard],
        "the segments move as one edit, so the layer scoped to both follows"
    );
    assert_eq!(
        rig.zone(rig.desk).await.layers[0].scope,
        Some(scope(&[KEYS, LOGO]))
    );
}

#[tokio::test]
async fn removing_a_member_leaves_its_layer_dormant() {
    let rig = rig().await;
    let layer_id = rig.add(rig.main, &[STRIP]).await;

    let response = send(
        &rig.app,
        Request::builder()
            .method("DELETE")
            .uri(format!("/api/v1/scene/zones/{}/members/{STRIP}", rig.main))
            .body(Body::empty())
            .expect("request"),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let zone = rig.zone(rig.main).await;
    let layer = zone
        .layers
        .iter()
        .find(|layer| layer.id == layer_id)
        .expect("the layer outlives its output");
    assert_eq!(layer.scope, Some(scope(&[STRIP])));
    assert!(zone.layer_is_dormant(layer));
}

// ── Apply and clear ──────────────────────────────────────────────────────

#[tokio::test]
async fn applying_an_effect_over_rest_keeps_scoped_layers() {
    let rig = rig().await;
    let keyboard = rig.add(rig.main, &[KEYS, LOGO]).await;
    let face = rig.add(rig.main, &[SCREEN]).await;
    let aurora = register(&rig.state, "Aurora").await;

    let response = send(
        &rig.app,
        json_request(
            "POST",
            format!("/api/v1/effects/{}/apply", aurora.id),
            &json!({}),
        ),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let zone = rig.zone(rig.main).await;
    assert_eq!(zone.layers.len(), 3);
    assert_eq!(zone.layers[0].scope, None);
    assert_eq!(zone.effect_ids().next(), Some(aurora.id));
    assert_eq!(
        zone.layers[1..]
            .iter()
            .map(|layer| layer.id)
            .collect::<Vec<_>>(),
        vec![keyboard, face]
    );
}

#[tokio::test]
async fn the_mcp_effect_and_clear_tools_follow_the_same_rules() {
    let rig = rig().await;
    let keyboard = rig.add(rig.main, &[KEYS, LOGO]).await;
    let face = rig.add(rig.main, &[SCREEN]).await;
    register(&rig.state, "Aurora").await;

    execute_tool_with_state(
        "set_effect",
        &json!({"query": "aurora"}),
        rig.state.as_ref(),
    )
    .await
    .expect("set_effect");
    assert_eq!(rig.stack(rig.main).await[1..], [keyboard, face]);

    execute_tool_with_state("clear_zone", &json!({}), rig.state.as_ref())
        .await
        .expect("clear_zone");
    assert_eq!(
        rig.stack(rig.main).await,
        vec![face],
        "clear stops lighting and leaves the screen's layer"
    );
}

#[tokio::test]
async fn clearing_the_scene_removes_lighting_and_keeps_screen_layers() {
    let rig = rig().await;
    rig.add(rig.main, &[KEYS, LOGO]).await;
    let face = rig.add(rig.main, &[SCREEN]).await;
    rig.add(rig.main, &[SCREEN, STRIP]).await;

    let response = send(
        &rig.app,
        json_request("POST", "/api/v1/scene/clear".into(), &json!({})),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(rig.stack(rig.main).await, vec![face]);

    // One zone, named: the same rule.
    rig.add(rig.main, &[STRIP]).await;
    let response = send(
        &rig.app,
        json_request(
            "POST",
            "/api/v1/scene/clear".into(),
            &json!({"zone": rig.main.to_string()}),
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(rig.stack(rig.main).await, vec![face]);
}

#[tokio::test]
async fn setting_a_zone_color_keeps_scoped_layers() {
    use hypercolor_daemon::domain::runtime_zone::{RuntimeZoneTarget, set_color};

    let rig = rig().await;
    let keyboard = rig.add(rig.main, &[KEYS, LOGO]).await;

    let outcome = set_color(
        &rig.state.domains.scene,
        RuntimeZoneTarget {
            scene_id: SceneId::DEFAULT,
            zone_id: rig.main,
            revision: rig.revision(),
        },
        hypercolor_color::Rgb::WHITE,
    )
    .await
    .expect("set a zone color");

    assert_eq!(outcome.zone.layers.len(), 2);
    assert_eq!(outcome.zone.layers[0].scope, None);
    assert_eq!(outcome.zone.layers[1].id, keyboard);
}

#[tokio::test]
async fn the_error_fallback_removes_a_failed_layer_that_clear_would_keep() {
    let rig = rig().await;
    let face = scoped_layer(&rig.bubbles, &[SCREEN]);
    let healthy_face = rig.add(rig.main, &[SCREEN]).await;
    let mut mutation = rig.state.scene_manager.begin_mutation().await;
    mutation
        .insert_layer(SceneId::DEFAULT, rig.main, face, None, None)
        .expect("a face running the failing effect");
    commit_scene(&rig.state.domains.scene, mutation)
        .await
        .expect("face commit");

    let applied = hypercolor_daemon::domain::effect::apply_error_fallback(
        &rig.state.domains.effects,
        &rig.bubbles.id.to_string(),
        EffectErrorFallbackPolicy::ClearZones,
    )
    .await
    .expect("fallback")
    .expect("the primary zone ran the failed effect");

    assert_eq!(applied.cleared_zone_count, 1);
    assert_eq!(
        rig.stack(rig.main).await,
        vec![healthy_face],
        "the failed effect is gone from every layer, and the healthy face stays"
    );
}

// ── Whole-scene replacement ──────────────────────────────────────────────

#[tokio::test]
async fn replacing_a_scene_keeps_the_scope_of_the_layers_it_names() {
    let rig = rig().await;
    let keyboard = rig.add(rig.main, &[KEYS, LOGO]).await;
    let mut studio = hypercolor_core::scene::make_scene("Studio");
    studio.zones = rig.scene().await.zones;
    let scene_id = studio.id;
    let mut mutation = rig.state.scene_manager.begin_mutation().await;
    mutation.create_scene(studio).expect("named scene");
    commit_scene(&rig.state.domains.scene, mutation)
        .await
        .expect("scene commit");
    let read = send(
        &rig.app,
        Request::builder()
            .uri(format!("/api/v1/scenes/{scene_id}"))
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(read.status(), StatusCode::OK);
    let document =
        serde_json::from_value(body_json(read).await["data"].clone()).expect("scene document");
    let mut replacement = ReplaceSceneRequest::from(&document);
    let layers = &mut replacement.zones[0].layers;
    layers[1].opacity = 0.25;
    layers.push(hypercolor_types::api::scenes::ReplaceSceneLayerRequest {
        id: None,
        ..layers[1].clone()
    });

    let response = send(
        &rig.app,
        json_request(
            "PUT",
            format!("/api/v1/scenes/{scene_id}"),
            &json!(replacement),
        ),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let stored = rig.state.scene_manager.snapshot().await;
    let zone = &stored.get(&scene_id).expect("named scene").zones[0];
    assert_eq!(zone.layers[1].id, keyboard);
    assert!((zone.layers[1].opacity - 0.25).abs() < f32::EPSILON);
    assert_eq!(
        zone.layers[1].scope,
        Some(scope(&[KEYS, LOGO])),
        "the request shape has no scope, so a named layer keeps the stored one"
    );
    assert_eq!(zone.layers[2].scope, None, "a new layer starts whole-zone");
}

// ── Persistence ──────────────────────────────────────────────────────────

#[tokio::test]
async fn a_scoped_layer_is_persisted_behind_the_new_schemas_and_reloads() {
    let rig = rig().await;
    let keyboard = rig.add(rig.main, &[KEYS, LOGO]).await;
    let mut studio = hypercolor_core::scene::make_scene("Studio");
    studio.zones = rig.scene().await.zones;
    let scene_id = studio.id;
    let mut mutation = rig.state.scene_manager.begin_mutation().await;
    mutation.create_scene(studio).expect("named scene");
    commit_scene(&rig.state.domains.scene, mutation)
        .await
        .expect("scene commit");
    rig.state
        .scene_manager
        .save_snapshot()
        .await
        .expect("scene store save");
    rig.state.domains.scene.save_runtime_session().await;
    for path in [
        rig.state.data_dir.join("scenes.json"),
        rig.state.runtime_state_path.clone(),
    ] {
        hypercolor_daemon::persistence::AtomicFileWriter::new(&path)
            .expect("writer should open")
            .flush(std::time::Duration::from_secs(5))
            .expect("the write should land");
    }

    let scenes_path = rig.state.data_dir.join("scenes.json");
    let scenes: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&scenes_path).expect("scenes.json"))
            .expect("scene store JSON");
    assert_eq!(scenes["schema_version"], 3);
    assert_eq!(
        scenes["scenes"][scene_id.to_string()]["zones"][0]["layers"][1]["scope"],
        json!({"outputs": [KEYS, LOGO]})
    );
    let session: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&rig.state.runtime_state_path).expect("runtime-state.json"),
    )
    .expect("runtime session JSON");
    assert_eq!(session["schema_version"], 1);
    assert_eq!(
        session["default_scene_zones"][0]["layers"][1]["scope"],
        json!({"outputs": [KEYS, LOGO]})
    );

    let stored = hypercolor_daemon::scene_store::load(&scenes_path).expect("scene store reloads");
    let stored = stored.list().next().expect("the named scene");
    assert_eq!(stored.zones[0].layers[1].id, keyboard);
    assert_eq!(stored.zones[0].layers[1].scope, Some(scope(&[KEYS, LOGO])));
    let reloaded = hypercolor_daemon::runtime_state::load(&rig.state.runtime_state_path)
        .expect("runtime session reloads")
        .expect("runtime session exists");
    assert_eq!(
        reloaded.default_scene_zones[0].layers[1].scope,
        Some(scope(&[KEYS, LOGO]))
    );
}
