use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::{Duration, SystemTime};

use hypercolor_core::effect::{EffectPool, EffectRegistry, builtin::register_builtin_effects};
use hypercolor_core::input::InteractionData;
use hypercolor_types::audio::AudioData;
use hypercolor_types::canvas::{Canvas, Rgba};
use hypercolor_types::control::ControlValue;
use hypercolor_types::effect::{ControlBinding, EffectId};
use hypercolor_types::layer::{LayerSource, SceneLayer, SceneLayerId};
use hypercolor_types::scene::{Zone, ZoneId, ZoneRole};
use hypercolor_types::sensor::SystemSnapshot;
use hypercolor_types::spatial::{
    EdgeBehavior, LedTopology, NormalizedPosition, Output, SamplingMode, SpatialLayout,
    StripDirection,
};

fn registry_with_builtins() -> EffectRegistry {
    let mut registry = EffectRegistry::new(Vec::new());
    register_builtin_effects(&mut registry);
    registry
}

static EMPTY_SENSORS: LazyLock<SystemSnapshot> = LazyLock::new(SystemSnapshot::empty);

fn builtin_effect_id(registry: &EffectRegistry, stem: &str) -> EffectId {
    registry
        .iter()
        .find_map(|(id, entry)| (entry.metadata.source.source_stem() == Some(stem)).then_some(*id))
        .expect("builtin effect should be registered")
}

fn sample_layout() -> SpatialLayout {
    SpatialLayout {
        id: "pool-test".into(),
        name: "Pool Test".into(),
        description: None,
        canvas_width: 32,
        canvas_height: 16,
        zones: vec![Output {
            id: "desk:main".into(),
            name: "Desk".into(),
            device_id: "mock:device".into(),
            zone_name: None,
            position: NormalizedPosition::new(0.5, 0.5),
            size: NormalizedPosition::new(1.0, 1.0),
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
        }],
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        version: 1,
    }
}

fn render_zone(id: ZoneId, effect_id: EffectId) -> Zone {
    Zone {
        id,
        name: "Desk".into(),
        description: None,
        layers: vec![SceneLayer::from_effect(
            SceneLayerId::new(),
            effect_id,
            HashMap::new(),
            HashMap::new(),
            None,
        )],
        layout: sample_layout(),
        brightness: 1.0,
        enabled: true,
        color: None,
        display_target: None,
        role: ZoneRole::Custom,
        controls_version: 0,
        layers_version: 0,
    }
}

fn effect_layer(effect_id: EffectId, color: [f32; 4]) -> SceneLayer {
    SceneLayer::from_effect(
        SceneLayerId::new(),
        effect_id,
        HashMap::from([("color".into(), ControlValue::linear_color(color))]),
        HashMap::new(),
        None,
    )
}

fn set_effect_control(zone: &mut Zone, name: &str, value: ControlValue) {
    let controls = zone
        .layers
        .iter_mut()
        .find_map(|layer| match &mut layer.source {
            LayerSource::Effect { controls, .. } => Some(controls),
            _ => None,
        });
    controls
        .expect("fixture should store an effect layer")
        .insert(name.to_owned(), value);
}

fn set_effect_id(zone: &mut Zone, effect_id: EffectId) {
    let stored_effect_id = zone
        .layers
        .iter_mut()
        .find_map(|layer| match &mut layer.source {
            LayerSource::Effect { effect_id, .. } => Some(effect_id),
            _ => None,
        });
    *stored_effect_id.expect("fixture should store an effect layer") = effect_id;
}

fn set_effect_control_binding(zone: &mut Zone, name: &str, binding: ControlBinding) {
    let bindings = zone
        .layers
        .iter_mut()
        .find_map(|layer| match &mut layer.source {
            LayerSource::Effect {
                control_bindings, ..
            } => Some(control_bindings),
            _ => None,
        });
    bindings
        .expect("fixture should store an effect layer")
        .insert(name.to_owned(), binding);
}

fn top_left(canvas: &Canvas) -> Rgba {
    canvas.get_pixel(0, 0)
}

#[test]
fn effect_pool_reconciles_and_renders_zone_controls() {
    let registry = registry_with_builtins();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let zone_id = ZoneId::new();
    let mut zone = render_zone(zone_id, solid_id);
    set_effect_control(
        &mut zone,
        "color",
        ControlValue::linear_color([1.0, 0.0, 0.0, 1.0]),
    );

    let mut pool = EffectPool::new();
    pool.reconcile(&[zone.clone()], &registry, &HashMap::new())
        .expect("zone should reconcile");

    let mut canvas = Canvas::new(1, 1);
    pool.render_zone_into(
        &zone,
        0.016,
        &AudioData::silence(),
        &InteractionData::default(),
        None,
        &EMPTY_SENSORS,
        hypercolor_core::effect::FrameDataSources::default(),
        &mut canvas,
    )
    .expect("zone should render");

    assert_eq!(pool.slot_count(), 1);
    assert_eq!(top_left(&canvas), Rgba::new(255, 0, 0, 255));
}

#[test]
fn failed_effect_pool_preparation_preserves_live_slots() {
    let registry = registry_with_builtins();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let live_zone = render_zone(ZoneId::new(), solid_id);
    let missing_zone = render_zone(
        ZoneId::new(),
        EffectId::new(uuid::Uuid::from_u128(0xfeed_face)),
    );
    let mut pool = EffectPool::new();
    pool.reconcile(std::slice::from_ref(&live_zone), &registry, &HashMap::new())
        .expect("live zone should reconcile");

    let result = pool.prepare_reconcile(&[missing_zone], &registry, &HashMap::new());

    assert!(result.is_err());
    assert_eq!(pool.slot_count(), 1);
    let mut canvas = Canvas::new(1, 1);
    pool.render_zone_into(
        &live_zone,
        0.016,
        &AudioData::silence(),
        &InteractionData::default(),
        None,
        &EMPTY_SENSORS,
        hypercolor_core::effect::FrameDataSources::default(),
        &mut canvas,
    )
    .expect("live slot should remain renderable after rejected preparation");
}

#[test]
fn invalid_effect_control_is_rejected_before_live_state_changes() {
    let registry = registry_with_builtins();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let mut live_zone = render_zone(ZoneId::new(), solid_id);
    set_effect_control(
        &mut live_zone,
        "color",
        ControlValue::linear_color([1.0, 0.0, 0.0, 1.0]),
    );
    let mut candidate_zone = live_zone.clone();
    set_effect_control(&mut candidate_zone, "color", ControlValue::Bool(true));
    candidate_zone.layers_version += 1;
    let mut pool = EffectPool::new();
    pool.reconcile(std::slice::from_ref(&live_zone), &registry, &HashMap::new())
        .expect("live controls should reconcile");

    let result = pool.prepare_reconcile(
        std::slice::from_ref(&candidate_zone),
        &registry,
        &HashMap::new(),
    );

    assert!(result.is_err());
    let mut canvas = Canvas::new(1, 1);
    pool.render_zone_into(
        &live_zone,
        0.016,
        &AudioData::silence(),
        &InteractionData::default(),
        None,
        &EMPTY_SENSORS,
        hypercolor_core::effect::FrameDataSources::default(),
        &mut canvas,
    )
    .expect("rejected controls must not disturb the live renderer");
    assert_eq!(top_left(&canvas), Rgba::new(255, 0, 0, 255));
}

#[test]
fn effect_pool_rejects_non_projectable_values_even_for_unknown_keys() {
    let registry = registry_with_builtins();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let rejected_values = [
        serde_json::json!({"kind": "null"}),
        serde_json::json!({"kind": "secret_ref", "value": "token"}),
        serde_json::json!({"kind": "ip", "value": "127.0.0.1"}),
        serde_json::json!({"kind": "mac", "value": "01:23:45:67:89:ab"}),
        serde_json::json!({"kind": "duration", "value": 250}),
        serde_json::json!({"kind": "color_rgb", "value": {"r": 1, "g": 2, "b": 3}}),
        serde_json::json!({"kind": "color_rgba", "value": {"r": 1, "g": 2, "b": 3, "a": 4}}),
        serde_json::json!({"kind": "flags", "value": ["one"]}),
        serde_json::json!({"kind": "list", "value": [{"kind": "bool", "value": true}]}),
        serde_json::json!({"kind": "map", "value": {"one": {"kind": "bool", "value": true}}}),
        serde_json::json!({"kind": "unknown"}),
    ];

    for (index, raw) in rejected_values.into_iter().enumerate() {
        let mut zone = render_zone(ZoneId::new(), solid_id);
        set_effect_control(
            &mut zone,
            &format!("unknown_{index}"),
            serde_json::from_value(raw).expect("fixture should decode canonically"),
        );
        let pool = EffectPool::new();

        assert!(
            pool.prepare_reconcile(&[zone], &registry, &HashMap::new())
                .is_err(),
            "non-projectable fixture {index} entered a prepared effect pool"
        );
    }
}

#[test]
fn abandoned_prepared_effect_pool_keeps_live_slots_renderable() {
    let registry = registry_with_builtins();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let live_zone = render_zone(ZoneId::new(), solid_id);
    let candidate_zone = render_zone(ZoneId::new(), solid_id);
    let mut pool = EffectPool::new();
    pool.reconcile(std::slice::from_ref(&live_zone), &registry, &HashMap::new())
        .expect("live zone should reconcile");

    let prepared = pool
        .prepare_reconcile(&[candidate_zone], &registry, &HashMap::new())
        .expect("candidate should fully prepare");
    drop(prepared);

    assert_eq!(pool.slot_count(), 1);
    let mut canvas = Canvas::new(1, 1);
    pool.render_zone_into(
        &live_zone,
        0.016,
        &AudioData::silence(),
        &InteractionData::default(),
        None,
        &EMPTY_SENSORS,
        hypercolor_core::effect::FrameDataSources::default(),
        &mut canvas,
    )
    .expect("live slot should survive an abandoned prepared replacement");
}

#[test]
fn changed_controls_update_slot_only_when_prepared_pool_commits() {
    let registry = registry_with_builtins();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let mut live_zone = render_zone(ZoneId::new(), solid_id);
    set_effect_control(
        &mut live_zone,
        "color",
        ControlValue::linear_color([1.0, 0.0, 0.0, 1.0]),
    );
    let mut candidate_zone = live_zone.clone();
    set_effect_control(
        &mut candidate_zone,
        "color",
        ControlValue::linear_color([0.0, 0.0, 1.0, 1.0]),
    );
    candidate_zone.layers_version += 1;
    let mut pool = EffectPool::new();
    pool.reconcile(std::slice::from_ref(&live_zone), &registry, &HashMap::new())
        .expect("live zone should reconcile");

    let prepared = pool
        .prepare_reconcile(
            std::slice::from_ref(&candidate_zone),
            &registry,
            &HashMap::new(),
        )
        .expect("changed controls should prepare a slot update");
    let mut canvas = Canvas::new(1, 1);
    pool.render_zone_into(
        &live_zone,
        0.016,
        &AudioData::silence(),
        &InteractionData::default(),
        None,
        &EMPTY_SENSORS,
        hypercolor_core::effect::FrameDataSources::default(),
        &mut canvas,
    )
    .expect("live slot should remain unchanged during preparation");
    assert_eq!(top_left(&canvas), Rgba::new(255, 0, 0, 255));

    pool.commit_reconcile(prepared)
        .expect("prepared reconcile should commit");
    pool.render_zone_into(
        &candidate_zone,
        0.016,
        &AudioData::silence(),
        &InteractionData::default(),
        None,
        &EMPTY_SENSORS,
        hypercolor_core::effect::FrameDataSources::default(),
        &mut canvas,
    )
    .expect("committed control update should render");
    assert_eq!(top_left(&canvas), Rgba::new(0, 0, 255, 255));
}

#[test]
fn stale_prepared_pool_is_rejected_before_any_live_control_update() {
    let registry = registry_with_builtins();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let mut live_a = render_zone(ZoneId::new(), solid_id);
    let mut live_b = render_zone(ZoneId::new(), solid_id);
    set_effect_control(
        &mut live_a,
        "color",
        ControlValue::linear_color([1.0, 0.0, 0.0, 1.0]),
    );
    set_effect_control(
        &mut live_b,
        "color",
        ControlValue::linear_color([1.0, 0.0, 0.0, 1.0]),
    );
    let mut candidate_a = live_a.clone();
    let mut candidate_b = live_b.clone();
    set_effect_control(
        &mut candidate_a,
        "color",
        ControlValue::linear_color([0.0, 0.0, 1.0, 1.0]),
    );
    set_effect_control(
        &mut candidate_b,
        "color",
        ControlValue::linear_color([0.0, 1.0, 0.0, 1.0]),
    );
    candidate_a.layers_version += 1;
    candidate_b.layers_version += 1;

    let mut pool = EffectPool::new();
    pool.reconcile(
        &[live_a.clone(), live_b.clone()],
        &registry,
        &HashMap::new(),
    )
    .expect("live zones should reconcile");
    let prepared = pool
        .prepare_reconcile(&[candidate_a, candidate_b], &registry, &HashMap::new())
        .expect("candidate controls should prepare");
    pool.remove_zone(live_b.id);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        pool.commit_reconcile(prepared)
            .expect("prepared reconcile should commit");
    }));

    assert!(result.is_err());
    let mut canvas = Canvas::new(1, 1);
    pool.render_zone_into(
        &live_a,
        0.016,
        &AudioData::silence(),
        &InteractionData::default(),
        None,
        &EMPTY_SENSORS,
        hypercolor_core::effect::FrameDataSources::default(),
        &mut canvas,
    )
    .expect("untouched live slot should remain renderable");
    assert_eq!(top_left(&canvas), Rgba::new(255, 0, 0, 255));
}

#[test]
fn stale_preparation_rejects_same_key_renderer_replacement() {
    let registry = registry_with_builtins();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let rainbow_id = builtin_effect_id(&registry, "rainbow");
    let mut live_zone = render_zone(ZoneId::new(), solid_id);
    set_effect_control(
        &mut live_zone,
        "color",
        ControlValue::linear_color([1.0, 0.0, 0.0, 1.0]),
    );
    let mut stale_candidate = live_zone.clone();
    set_effect_control(
        &mut stale_candidate,
        "color",
        ControlValue::linear_color([0.0, 0.0, 1.0, 1.0]),
    );
    stale_candidate.layers_version += 1;

    let mut pool = EffectPool::new();
    pool.reconcile(std::slice::from_ref(&live_zone), &registry, &HashMap::new())
        .expect("live zone should reconcile");
    let stale = pool
        .prepare_reconcile(
            std::slice::from_ref(&stale_candidate),
            &registry,
            &HashMap::new(),
        )
        .expect("control update should prepare against the solid renderer");

    let mut replacement = live_zone.clone();
    set_effect_id(&mut replacement, rainbow_id);
    pool.reconcile(
        std::slice::from_ref(&replacement),
        &registry,
        &HashMap::new(),
    )
    .expect("same-key rainbow renderer should replace the solid renderer");

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = pool.commit_reconcile(stale);
    }));

    assert!(result.is_err());
    assert_eq!(pool.slot_count(), 1);
    let mut canvas = Canvas::new(1, 1);
    pool.render_zone_into(
        &replacement,
        0.016,
        &AudioData::silence(),
        &InteractionData::default(),
        None,
        &EMPTY_SENSORS,
        hypercolor_core::effect::FrameDataSources::default(),
        &mut canvas,
    )
    .expect("replacement renderer should remain live after stale commit rejection");
}

#[test]
fn effect_pool_hot_swaps_effects_for_same_zone() {
    let registry = registry_with_builtins();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let rainbow_id = builtin_effect_id(&registry, "rainbow");
    let zone_id = ZoneId::new();
    let mut solid_zone = render_zone(zone_id, solid_id);
    set_effect_control(
        &mut solid_zone,
        "color",
        ControlValue::linear_color([1.0, 0.0, 0.0, 1.0]),
    );

    let mut pool = EffectPool::new();
    pool.reconcile(
        std::slice::from_ref(&solid_zone),
        &registry,
        &HashMap::new(),
    )
    .expect("solid zone should reconcile");
    let mut solid_canvas = Canvas::new(1, 1);
    pool.render_zone_into(
        &solid_zone,
        0.016,
        &AudioData::silence(),
        &InteractionData::default(),
        None,
        &EMPTY_SENSORS,
        hypercolor_core::effect::FrameDataSources::default(),
        &mut solid_canvas,
    )
    .expect("solid zone should render");

    let rainbow_zone = render_zone(zone_id, rainbow_id);
    pool.reconcile(
        std::slice::from_ref(&rainbow_zone),
        &registry,
        &HashMap::new(),
    )
    .expect("rainbow zone should reconcile");
    let mut rainbow_canvas = Canvas::new(1, 1);
    pool.render_zone_into(
        &rainbow_zone,
        0.016,
        &AudioData::silence(),
        &InteractionData::default(),
        None,
        &EMPTY_SENSORS,
        hypercolor_core::effect::FrameDataSources::default(),
        &mut rainbow_canvas,
    )
    .expect("rainbow zone should render");

    assert_eq!(pool.slot_count(), 1);
    assert_ne!(top_left(&solid_canvas), top_left(&rainbow_canvas));
}

#[test]
fn effect_pool_rebuilds_slot_when_registry_entry_changes_for_same_effect_id() {
    let mut registry = registry_with_builtins();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let rainbow_entry = registry
        .iter()
        .find_map(|(_, entry)| {
            (entry.metadata.source.source_stem() == Some("rainbow")).then_some(entry.clone())
        })
        .expect("rainbow effect should be registered");
    let zone_id = ZoneId::new();
    let mut zone = render_zone(zone_id, solid_id);
    set_effect_control(
        &mut zone,
        "color",
        ControlValue::linear_color([1.0, 0.0, 0.0, 1.0]),
    );

    let mut pool = EffectPool::new();
    pool.reconcile(std::slice::from_ref(&zone), &registry, &HashMap::new())
        .expect("initial zone should reconcile");

    let mut before_reload = Canvas::new(1, 1);
    pool.render_zone_into(
        &zone,
        0.016,
        &AudioData::silence(),
        &InteractionData::default(),
        None,
        &EMPTY_SENSORS,
        hypercolor_core::effect::FrameDataSources::default(),
        &mut before_reload,
    )
    .expect("solid effect should render");

    let mut replacement = rainbow_entry;
    replacement.metadata.id = solid_id;
    registry.register(replacement);

    pool.reconcile(std::slice::from_ref(&zone), &registry, &HashMap::new())
        .expect("registry change should trigger rebuild");

    let mut after_reload = Canvas::new(1, 1);
    pool.render_zone_into(
        &zone,
        0.016,
        &AudioData::silence(),
        &InteractionData::default(),
        None,
        &EMPTY_SENSORS,
        hypercolor_core::effect::FrameDataSources::default(),
        &mut after_reload,
    )
    .expect("reloaded effect should render");

    assert_eq!(top_left(&before_reload), Rgba::new(255, 0, 0, 255));
    assert_ne!(top_left(&after_reload), top_left(&before_reload));
}

#[test]
fn effect_pool_rebuilds_slot_when_registry_modified_changes_for_same_effect_id() {
    let mut registry = registry_with_builtins();
    let rainbow_id = builtin_effect_id(&registry, "rainbow");
    let zone = render_zone(ZoneId::new(), rainbow_id);

    let mut pool = EffectPool::new();
    pool.reconcile(std::slice::from_ref(&zone), &registry, &HashMap::new())
        .expect("initial zone should reconcile");

    let mut before_reload = Canvas::new(1, 1);
    pool.render_zone_into(
        &zone,
        0.5,
        &AudioData::silence(),
        &InteractionData::default(),
        None,
        &EMPTY_SENSORS,
        hypercolor_core::effect::FrameDataSources::default(),
        &mut before_reload,
    )
    .expect("rainbow effect should render before reload");

    let mut updated_entry = registry
        .get(&rainbow_id)
        .expect("rainbow entry should exist")
        .clone();
    updated_entry.modified = SystemTime::UNIX_EPOCH + Duration::from_secs(1);
    registry.register(updated_entry);

    pool.reconcile(std::slice::from_ref(&zone), &registry, &HashMap::new())
        .expect("modified timestamp change should trigger rebuild");

    let mut after_reload = Canvas::new(1, 1);
    pool.render_zone_into(
        &zone,
        0.5,
        &AudioData::silence(),
        &InteractionData::default(),
        None,
        &EMPTY_SENSORS,
        hypercolor_core::effect::FrameDataSources::default(),
        &mut after_reload,
    )
    .expect("rainbow effect should render after reload");

    assert_eq!(
        top_left(&after_reload),
        top_left(&before_reload),
        "rebuilding on modified changes should reset the renderer timeline"
    );
}

#[test]
fn effect_pool_does_not_rebuild_slot_for_control_binding_state() {
    let registry = registry_with_builtins();
    let rainbow_id = builtin_effect_id(&registry, "rainbow");
    let mut zone = render_zone(ZoneId::new(), rainbow_id);
    let bound_control_id = registry
        .get(&rainbow_id)
        .and_then(|entry| entry.metadata.controls.first())
        .map(|control| control.control_id().to_owned())
        .expect("rainbow should expose at least one control");
    set_effect_control_binding(
        &mut zone,
        &bound_control_id,
        ControlBinding {
            sensor: "cpu_temp".into(),
            sensor_min: 0.0,
            sensor_max: 100.0,
            target_min: 0.0,
            target_max: 1.0,
            deadband: 0.0,
            smoothing: 0.0,
        },
    );

    let mut pool = EffectPool::new();
    pool.reconcile(std::slice::from_ref(&zone), &registry, &HashMap::new())
        .expect("bound zone should reconcile");

    let mut first = Canvas::new(1, 1);
    pool.render_zone_into(
        &zone,
        0.5,
        &AudioData::silence(),
        &InteractionData::default(),
        None,
        &EMPTY_SENSORS,
        hypercolor_core::effect::FrameDataSources::default(),
        &mut first,
    )
    .expect("first rainbow frame should render");

    pool.reconcile(std::slice::from_ref(&zone), &registry, &HashMap::new())
        .expect("stable registry metadata should not force rebuild");

    let mut second = Canvas::new(1, 1);
    pool.render_zone_into(
        &zone,
        0.5,
        &AudioData::silence(),
        &InteractionData::default(),
        None,
        &EMPTY_SENSORS,
        hypercolor_core::effect::FrameDataSources::default(),
        &mut second,
    )
    .expect("second rainbow frame should render");

    assert_ne!(
        top_left(&second),
        top_left(&first),
        "binding state should not reset renderer timeline on reconcile"
    );
}

#[test]
fn effect_pool_prunes_removed_zones() {
    let registry = registry_with_builtins();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let zone = render_zone(ZoneId::new(), solid_id);

    let mut pool = EffectPool::new();
    pool.reconcile(&[zone], &registry, &HashMap::new())
        .expect("zone should reconcile");
    assert_eq!(pool.slot_count(), 1);

    pool.reconcile(&[], &registry, &HashMap::new())
        .expect("empty zone list should prune");
    assert_eq!(pool.slot_count(), 0);
}

#[test]
fn effect_pool_prunes_disabled_zones() {
    let registry = registry_with_builtins();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let zone_id = ZoneId::new();
    let enabled_zone = render_zone(zone_id, solid_id);
    let mut disabled_zone = render_zone(zone_id, solid_id);
    disabled_zone.enabled = false;

    let mut pool = EffectPool::new();
    pool.reconcile(&[enabled_zone], &registry, &HashMap::new())
        .expect("enabled zone should reconcile");

    let mut canvas = Canvas::new(1, 1);
    canvas.fill(Rgba::new(255, 0, 0, 255));
    pool.reconcile(&[disabled_zone.clone()], &registry, &HashMap::new())
        .expect("disabled zone should still reconcile");
    pool.render_zone_into(
        &disabled_zone,
        0.016,
        &AudioData::silence(),
        &InteractionData::default(),
        None,
        &EMPTY_SENSORS,
        hypercolor_core::effect::FrameDataSources::default(),
        &mut canvas,
    )
    .expect("disabled zone should clear");

    assert_eq!(pool.slot_count(), 0);
    assert_eq!(top_left(&canvas), Rgba::new(0, 0, 0, 255));
}

#[test]
fn effect_pool_reconciles_duplicate_effect_layers_as_separate_slots() {
    let registry = registry_with_builtins();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let zone_id = ZoneId::new();
    let red_layer = effect_layer(solid_id, [1.0, 0.0, 0.0, 1.0]);
    let blue_layer = effect_layer(solid_id, [0.0, 0.0, 1.0, 1.0]);
    let mut zone = render_zone(zone_id, solid_id);
    zone.layers = vec![red_layer.clone(), blue_layer.clone()];

    let mut pool = EffectPool::new();
    pool.reconcile(std::slice::from_ref(&zone), &registry, &HashMap::new())
        .expect("layered zone should reconcile");

    let mut red_canvas = Canvas::new(1, 1);
    pool.render_layer_into(
        &zone,
        &red_layer,
        0.016,
        &AudioData::silence(),
        &InteractionData::default(),
        None,
        &EMPTY_SENSORS,
        hypercolor_core::effect::FrameDataSources::default(),
        &mut red_canvas,
    )
    .expect("red layer should render");

    let mut blue_canvas = Canvas::new(1, 1);
    pool.render_layer_into(
        &zone,
        &blue_layer,
        0.016,
        &AudioData::silence(),
        &InteractionData::default(),
        None,
        &EMPTY_SENSORS,
        hypercolor_core::effect::FrameDataSources::default(),
        &mut blue_canvas,
    )
    .expect("blue layer should render");

    assert_eq!(pool.slot_count(), 2);
    assert_eq!(top_left(&red_canvas), Rgba::new(255, 0, 0, 255));
    assert_eq!(top_left(&blue_canvas), Rgba::new(0, 0, 255, 255));
}

#[test]
fn effect_pool_skips_disabled_effect_layers() {
    let registry = registry_with_builtins();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let mut zone = render_zone(ZoneId::new(), solid_id);
    let mut disabled_layer = effect_layer(solid_id, [0.0, 0.0, 1.0, 1.0]);
    disabled_layer.enabled = false;
    zone.layers = vec![effect_layer(solid_id, [1.0, 0.0, 0.0, 1.0]), disabled_layer];

    let mut pool = EffectPool::new();
    pool.reconcile(std::slice::from_ref(&zone), &registry, &HashMap::new())
        .expect("enabled layer should reconcile");

    assert_eq!(pool.slot_count(), 1);
}
