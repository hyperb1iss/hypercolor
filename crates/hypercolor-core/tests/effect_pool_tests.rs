use std::collections::HashMap;

use hypercolor_core::effect::{EffectPool, EffectRegistry, builtin::register_builtin_effects};
use hypercolor_core::input::InteractionData;
use hypercolor_types::audio::AudioData;
use hypercolor_types::canvas::{Canvas, Rgba};
use hypercolor_types::effect::{ControlValue, EffectId};
use hypercolor_types::scene::{RenderGroup, RenderGroupId};
use hypercolor_types::spatial::{
    DeviceZone, EdgeBehavior, LedTopology, NormalizedPosition, SamplingMode, SpatialLayout,
    StripDirection,
};

fn registry_with_builtins() -> EffectRegistry {
    let mut registry = EffectRegistry::new(Vec::new());
    register_builtin_effects(&mut registry);
    registry
}

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
        preset_id: None,
        layout: sample_layout(),
        brightness: 1.0,
        enabled: true,
        color: None,
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
        &mut rainbow_canvas,
    )
    .expect("rainbow group should render");

    assert_eq!(pool.slot_count(), 1);
    assert_ne!(top_left(&solid_canvas), top_left(&rainbow_canvas));
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
        &mut canvas,
    )
    .expect("disabled group should clear");

    assert_eq!(pool.slot_count(), 1);
    assert_eq!(top_left(&canvas), Rgba::new(0, 0, 0, 255));
}
