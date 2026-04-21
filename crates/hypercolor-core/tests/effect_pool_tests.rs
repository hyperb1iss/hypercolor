use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::{Duration, SystemTime};

use hypercolor_core::effect::{EffectPool, EffectRegistry, builtin::register_builtin_effects};
use hypercolor_core::input::InteractionData;
use hypercolor_types::audio::AudioData;
use hypercolor_types::canvas::{Canvas, Rgba};
use hypercolor_types::effect::{ControlBinding, ControlValue, EffectId};
use hypercolor_types::scene::{RenderGroup, RenderGroupId, RenderGroupRole};
use hypercolor_types::sensor::SystemSnapshot;
use hypercolor_types::spatial::{
    DeviceZone, EdgeBehavior, LedTopology, NormalizedPosition, SamplingMode, SpatialLayout,
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
        zones: vec![DeviceZone {
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

fn render_group(id: RenderGroupId, effect_id: EffectId) -> RenderGroup {
    RenderGroup {
        id,
        name: "Desk".into(),
        description: None,
        effect_id: Some(effect_id),
        controls: HashMap::new(),
        control_bindings: HashMap::new(),
        preset_id: None,
        layout: sample_layout(),
        brightness: 1.0,
        enabled: true,
        color: None,
        display_target: None,
        role: RenderGroupRole::Custom,
        controls_version: 0,
    }
}

fn top_left(canvas: &Canvas) -> Rgba {
    canvas.get_pixel(0, 0)
}

#[test]
fn effect_pool_reconciles_and_renders_group_controls() {
    let registry = registry_with_builtins();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let group_id = RenderGroupId::new();
    let mut group = render_group(group_id, solid_id);
    group
        .controls
        .insert("color".into(), ControlValue::Color([1.0, 0.0, 0.0, 1.0]));

    let mut pool = EffectPool::new();
    pool.reconcile(&[group.clone()], &registry)
        .expect("group should reconcile");

    let mut canvas = Canvas::new(1, 1);
    pool.render_group_into(
        &group,
        0.016,
        &AudioData::silence(),
        &InteractionData::default(),
        None,
        &EMPTY_SENSORS,
        &mut canvas,
    )
    .expect("group should render");

    assert_eq!(pool.slot_count(), 1);
    assert_eq!(top_left(&canvas), Rgba::new(255, 0, 0, 255));
}

#[test]
fn effect_pool_hot_swaps_effects_for_same_group() {
    let registry = registry_with_builtins();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let rainbow_id = builtin_effect_id(&registry, "rainbow");
    let group_id = RenderGroupId::new();
    let mut solid_group = render_group(group_id, solid_id);
    solid_group
        .controls
        .insert("color".into(), ControlValue::Color([1.0, 0.0, 0.0, 1.0]));

    let mut pool = EffectPool::new();
    pool.reconcile(std::slice::from_ref(&solid_group), &registry)
        .expect("solid group should reconcile");
    let mut solid_canvas = Canvas::new(1, 1);
    pool.render_group_into(
        &solid_group,
        0.016,
        &AudioData::silence(),
        &InteractionData::default(),
        None,
        &EMPTY_SENSORS,
        &mut solid_canvas,
    )
    .expect("solid group should render");

    let rainbow_group = render_group(group_id, rainbow_id);
    pool.reconcile(std::slice::from_ref(&rainbow_group), &registry)
        .expect("rainbow group should reconcile");
    let mut rainbow_canvas = Canvas::new(1, 1);
    pool.render_group_into(
        &rainbow_group,
        0.016,
        &AudioData::silence(),
        &InteractionData::default(),
        None,
        &EMPTY_SENSORS,
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
    let group_id = RenderGroupId::new();
    let mut group = render_group(group_id, solid_id);
    group
        .controls
        .insert("color".into(), ControlValue::Color([1.0, 0.0, 0.0, 1.0]));

    let mut pool = EffectPool::new();
    pool.reconcile(std::slice::from_ref(&group), &registry)
        .expect("initial group should reconcile");

    let mut before_reload = Canvas::new(1, 1);
    pool.render_group_into(
        &group,
        0.016,
        &AudioData::silence(),
        &InteractionData::default(),
        None,
        &EMPTY_SENSORS,
        &mut before_reload,
    )
    .expect("solid effect should render");

    let mut replacement = rainbow_entry;
    replacement.metadata.id = solid_id;
    registry.register(replacement);

    pool.reconcile(std::slice::from_ref(&group), &registry)
        .expect("registry change should trigger rebuild");

    let mut after_reload = Canvas::new(1, 1);
    pool.render_group_into(
        &group,
        0.016,
        &AudioData::silence(),
        &InteractionData::default(),
        None,
        &EMPTY_SENSORS,
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
    let group = render_group(RenderGroupId::new(), rainbow_id);

    let mut pool = EffectPool::new();
    pool.reconcile(std::slice::from_ref(&group), &registry)
        .expect("initial group should reconcile");

    let mut before_reload = Canvas::new(1, 1);
    pool.render_group_into(
        &group,
        0.5,
        &AudioData::silence(),
        &InteractionData::default(),
        None,
        &EMPTY_SENSORS,
        &mut before_reload,
    )
    .expect("rainbow effect should render before reload");

    let mut updated_entry = registry
        .get(&rainbow_id)
        .expect("rainbow entry should exist")
        .clone();
    updated_entry.modified = SystemTime::UNIX_EPOCH + Duration::from_secs(1);
    registry.register(updated_entry);

    pool.reconcile(std::slice::from_ref(&group), &registry)
        .expect("modified timestamp change should trigger rebuild");

    let mut after_reload = Canvas::new(1, 1);
    pool.render_group_into(
        &group,
        0.5,
        &AudioData::silence(),
        &InteractionData::default(),
        None,
        &EMPTY_SENSORS,
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
    let mut group = render_group(RenderGroupId::new(), rainbow_id);
    let bound_control_id = registry
        .get(&rainbow_id)
        .and_then(|entry| entry.metadata.controls.first())
        .map(|control| control.control_id().to_owned())
        .expect("rainbow should expose at least one control");
    group.control_bindings.insert(
        bound_control_id,
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
    pool.reconcile(std::slice::from_ref(&group), &registry)
        .expect("bound group should reconcile");

    let mut first = Canvas::new(1, 1);
    pool.render_group_into(
        &group,
        0.5,
        &AudioData::silence(),
        &InteractionData::default(),
        None,
        &EMPTY_SENSORS,
        &mut first,
    )
    .expect("first rainbow frame should render");

    pool.reconcile(std::slice::from_ref(&group), &registry)
        .expect("stable registry metadata should not force rebuild");

    let mut second = Canvas::new(1, 1);
    pool.render_group_into(
        &group,
        0.5,
        &AudioData::silence(),
        &InteractionData::default(),
        None,
        &EMPTY_SENSORS,
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
    let group = render_group(RenderGroupId::new(), solid_id);

    let mut pool = EffectPool::new();
    pool.reconcile(&[group], &registry)
        .expect("group should reconcile");
    assert_eq!(pool.slot_count(), 1);

    pool.reconcile(&[], &registry)
        .expect("empty group list should prune");
    assert_eq!(pool.slot_count(), 0);
}

#[test]
fn effect_pool_clears_disabled_groups_without_dropping_slots() {
    let registry = registry_with_builtins();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let group_id = RenderGroupId::new();
    let enabled_group = render_group(group_id, solid_id);
    let mut disabled_group = render_group(group_id, solid_id);
    disabled_group.enabled = false;

    let mut pool = EffectPool::new();
    pool.reconcile(&[enabled_group], &registry)
        .expect("enabled group should reconcile");

    let mut canvas = Canvas::new(1, 1);
    canvas.fill(Rgba::new(255, 0, 0, 255));
    pool.reconcile(&[disabled_group.clone()], &registry)
        .expect("disabled group should still reconcile");
    pool.render_group_into(
        &disabled_group,
        0.016,
        &AudioData::silence(),
        &InteractionData::default(),
        None,
        &EMPTY_SENSORS,
        &mut canvas,
    )
    .expect("disabled group should clear");

    assert_eq!(pool.slot_count(), 1);
    assert_eq!(top_left(&canvas), Rgba::new(0, 0, 0, 255));
}
