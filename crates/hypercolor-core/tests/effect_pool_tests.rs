use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::{Duration, SystemTime};

use hypercolor_core::effect::{EffectPool, EffectRegistry, builtin::register_builtin_effects};
use hypercolor_core::input::InteractionData;
use hypercolor_types::audio::AudioData;
use hypercolor_types::canvas::{Canvas, Rgba};
use hypercolor_types::effect::{ControlBinding, ControlValue, EffectId};
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
        spaces: None,
        version: 1,
    }
}

fn render_group(id: ZoneId, effect_id: EffectId) -> Zone {
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
        HashMap::from([("color".into(), ControlValue::Color(color))]),
        HashMap::new(),
        None,
    )
}

fn set_effect_control(group: &mut Zone, name: &str, value: ControlValue) {
    let controls = group
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

fn set_effect_control_binding(group: &mut Zone, name: &str, binding: ControlBinding) {
    let bindings = group
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
fn effect_pool_reconciles_and_renders_group_controls() {
    let registry = registry_with_builtins();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let group_id = ZoneId::new();
    let mut group = render_group(group_id, solid_id);
    set_effect_control(
        &mut group,
        "color",
        ControlValue::Color([1.0, 0.0, 0.0, 1.0]),
    );

    let mut pool = EffectPool::new();
    pool.reconcile(&[group.clone()], &registry, &HashMap::new())
        .expect("group should reconcile");

    let mut canvas = Canvas::new(1, 1);
    pool.render_group_into(
        &group,
        0.016,
        &AudioData::silence(),
        &InteractionData::default(),
        None,
        &EMPTY_SENSORS,
        hypercolor_core::effect::FrameDataSources::default(),
        &mut canvas,
    )
    .expect("group should render");

    assert_eq!(pool.slot_count(), 1);
    assert_eq!(top_left(&canvas), Rgba::new(255, 0, 0, 255));
}

#[test]
fn failed_effect_pool_preparation_preserves_live_slots() {
    let registry = registry_with_builtins();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let live_group = render_group(ZoneId::new(), solid_id);
    let missing_group = render_group(
        ZoneId::new(),
        EffectId::new(uuid::Uuid::from_u128(0xfeed_face)),
    );
    let mut pool = EffectPool::new();
    pool.reconcile(
        std::slice::from_ref(&live_group),
        &registry,
        &HashMap::new(),
    )
    .expect("live group should reconcile");

    let result = pool.prepare_reconcile(&[missing_group], &registry, &HashMap::new());

    assert!(result.is_err());
    assert_eq!(pool.slot_count(), 1);
    let mut canvas = Canvas::new(1, 1);
    pool.render_group_into(
        &live_group,
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
fn abandoned_prepared_effect_pool_keeps_live_slots_renderable() {
    let registry = registry_with_builtins();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let live_group = render_group(ZoneId::new(), solid_id);
    let candidate_group = render_group(ZoneId::new(), solid_id);
    let mut pool = EffectPool::new();
    pool.reconcile(
        std::slice::from_ref(&live_group),
        &registry,
        &HashMap::new(),
    )
    .expect("live group should reconcile");

    let prepared = pool
        .prepare_reconcile(&[candidate_group], &registry, &HashMap::new())
        .expect("candidate should fully prepare");
    drop(prepared);

    assert_eq!(pool.slot_count(), 1);
    let mut canvas = Canvas::new(1, 1);
    pool.render_group_into(
        &live_group,
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
    let mut live_group = render_group(ZoneId::new(), solid_id);
    set_effect_control(
        &mut live_group,
        "color",
        ControlValue::Color([1.0, 0.0, 0.0, 1.0]),
    );
    let mut candidate_group = live_group.clone();
    set_effect_control(
        &mut candidate_group,
        "color",
        ControlValue::Color([0.0, 0.0, 1.0, 1.0]),
    );
    candidate_group.controls_version += 1;
    let mut pool = EffectPool::new();
    pool.reconcile(
        std::slice::from_ref(&live_group),
        &registry,
        &HashMap::new(),
    )
    .expect("live group should reconcile");

    let prepared = pool
        .prepare_reconcile(
            std::slice::from_ref(&candidate_group),
            &registry,
            &HashMap::new(),
        )
        .expect("changed controls should prepare a slot update");
    let mut canvas = Canvas::new(1, 1);
    pool.render_group_into(
        &live_group,
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

    pool.commit_reconcile(prepared).expect("commit reconcile");
    pool.render_group_into(
        &candidate_group,
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
fn effect_pool_hot_swaps_effects_for_same_group() {
    let registry = registry_with_builtins();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let rainbow_id = builtin_effect_id(&registry, "rainbow");
    let group_id = ZoneId::new();
    let mut solid_group = render_group(group_id, solid_id);
    set_effect_control(
        &mut solid_group,
        "color",
        ControlValue::Color([1.0, 0.0, 0.0, 1.0]),
    );

    let mut pool = EffectPool::new();
    pool.reconcile(
        std::slice::from_ref(&solid_group),
        &registry,
        &HashMap::new(),
    )
    .expect("solid group should reconcile");
    let mut solid_canvas = Canvas::new(1, 1);
    pool.render_group_into(
        &solid_group,
        0.016,
        &AudioData::silence(),
        &InteractionData::default(),
        None,
        &EMPTY_SENSORS,
        hypercolor_core::effect::FrameDataSources::default(),
        &mut solid_canvas,
    )
    .expect("solid group should render");

    let rainbow_group = render_group(group_id, rainbow_id);
    pool.reconcile(
        std::slice::from_ref(&rainbow_group),
        &registry,
        &HashMap::new(),
    )
    .expect("rainbow group should reconcile");
    let mut rainbow_canvas = Canvas::new(1, 1);
    pool.render_group_into(
        &rainbow_group,
        0.016,
        &AudioData::silence(),
        &InteractionData::default(),
        None,
        &EMPTY_SENSORS,
        hypercolor_core::effect::FrameDataSources::default(),
        &mut rainbow_canvas,
    )
    .expect("rainbow group should render");

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
    let group_id = ZoneId::new();
    let mut group = render_group(group_id, solid_id);
    set_effect_control(
        &mut group,
        "color",
        ControlValue::Color([1.0, 0.0, 0.0, 1.0]),
    );

    let mut pool = EffectPool::new();
    pool.reconcile(std::slice::from_ref(&group), &registry, &HashMap::new())
        .expect("initial group should reconcile");

    let mut before_reload = Canvas::new(1, 1);
    pool.render_group_into(
        &group,
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

    pool.reconcile(std::slice::from_ref(&group), &registry, &HashMap::new())
        .expect("registry change should trigger rebuild");

    let mut after_reload = Canvas::new(1, 1);
    pool.render_group_into(
        &group,
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
    let group = render_group(ZoneId::new(), rainbow_id);

    let mut pool = EffectPool::new();
    pool.reconcile(std::slice::from_ref(&group), &registry, &HashMap::new())
        .expect("initial group should reconcile");

    let mut before_reload = Canvas::new(1, 1);
    pool.render_group_into(
        &group,
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

    pool.reconcile(std::slice::from_ref(&group), &registry, &HashMap::new())
        .expect("modified timestamp change should trigger rebuild");

    let mut after_reload = Canvas::new(1, 1);
    pool.render_group_into(
        &group,
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
    let mut group = render_group(ZoneId::new(), rainbow_id);
    let bound_control_id = registry
        .get(&rainbow_id)
        .and_then(|entry| entry.metadata.controls.first())
        .map(|control| control.control_id().to_owned())
        .expect("rainbow should expose at least one control");
    set_effect_control_binding(
        &mut group,
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
    pool.reconcile(std::slice::from_ref(&group), &registry, &HashMap::new())
        .expect("bound group should reconcile");

    let mut first = Canvas::new(1, 1);
    pool.render_group_into(
        &group,
        0.5,
        &AudioData::silence(),
        &InteractionData::default(),
        None,
        &EMPTY_SENSORS,
        hypercolor_core::effect::FrameDataSources::default(),
        &mut first,
    )
    .expect("first rainbow frame should render");

    pool.reconcile(std::slice::from_ref(&group), &registry, &HashMap::new())
        .expect("stable registry metadata should not force rebuild");

    let mut second = Canvas::new(1, 1);
    pool.render_group_into(
        &group,
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
fn effect_pool_prunes_removed_groups() {
    let registry = registry_with_builtins();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let group = render_group(ZoneId::new(), solid_id);

    let mut pool = EffectPool::new();
    pool.reconcile(&[group], &registry, &HashMap::new())
        .expect("group should reconcile");
    assert_eq!(pool.slot_count(), 1);

    pool.reconcile(&[], &registry, &HashMap::new())
        .expect("empty group list should prune");
    assert_eq!(pool.slot_count(), 0);
}

#[test]
fn effect_pool_prunes_disabled_groups() {
    let registry = registry_with_builtins();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let group_id = ZoneId::new();
    let enabled_group = render_group(group_id, solid_id);
    let mut disabled_group = render_group(group_id, solid_id);
    disabled_group.enabled = false;

    let mut pool = EffectPool::new();
    pool.reconcile(&[enabled_group], &registry, &HashMap::new())
        .expect("enabled group should reconcile");

    let mut canvas = Canvas::new(1, 1);
    canvas.fill(Rgba::new(255, 0, 0, 255));
    pool.reconcile(&[disabled_group.clone()], &registry, &HashMap::new())
        .expect("disabled group should still reconcile");
    pool.render_group_into(
        &disabled_group,
        0.016,
        &AudioData::silence(),
        &InteractionData::default(),
        None,
        &EMPTY_SENSORS,
        hypercolor_core::effect::FrameDataSources::default(),
        &mut canvas,
    )
    .expect("disabled group should clear");

    assert_eq!(pool.slot_count(), 0);
    assert_eq!(top_left(&canvas), Rgba::new(0, 0, 0, 255));
}

#[test]
fn effect_pool_reconciles_duplicate_effect_layers_as_separate_slots() {
    let registry = registry_with_builtins();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let group_id = ZoneId::new();
    let red_layer = effect_layer(solid_id, [1.0, 0.0, 0.0, 1.0]);
    let blue_layer = effect_layer(solid_id, [0.0, 0.0, 1.0, 1.0]);
    let mut group = render_group(group_id, solid_id);
    group.layers = vec![red_layer.clone(), blue_layer.clone()];

    let mut pool = EffectPool::new();
    pool.reconcile(std::slice::from_ref(&group), &registry, &HashMap::new())
        .expect("layered group should reconcile");

    let mut red_canvas = Canvas::new(1, 1);
    pool.render_layer_into(
        &group,
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
        &group,
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
    let mut group = render_group(ZoneId::new(), solid_id);
    let mut disabled_layer = effect_layer(solid_id, [0.0, 0.0, 1.0, 1.0]);
    disabled_layer.enabled = false;
    group.layers = vec![effect_layer(solid_id, [1.0, 0.0, 0.0, 1.0]), disabled_layer];

    let mut pool = EffectPool::new();
    pool.reconcile(std::slice::from_ref(&group), &registry, &HashMap::new())
        .expect("enabled layer should reconcile");

    assert_eq!(pool.slot_count(), 1);
}
