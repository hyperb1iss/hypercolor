//! Tests for the scene engine: manager, transitions, priority stack, and automation.

use std::collections::HashMap;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use hypercolor_core::scene::automation::AutomationEngine;
use hypercolor_core::scene::priority::PriorityStack;
use hypercolor_core::scene::{LayerMutationError, SceneManager, make_scene};
use hypercolor_types::canvas::LinearRgba;
use hypercolor_types::effect::{
    ControlValue, EffectCategory, EffectId, EffectMetadata, EffectSource,
};
use hypercolor_types::layer::{
    LayerAdjust, LayerBlendMode, LayerSource, LayerTransform, SceneLayer, SceneLayerId,
};
use hypercolor_types::scene::{
    ActionKind, AutomationRule, ColorInterpolation, EasingFunction, SceneId, ScenePriority,
    TransitionSpec, TriggerSource, UnassignedBehavior, Zone, ZoneId, ZoneRole,
};
use hypercolor_types::spatial::{
    EdgeBehavior, LedTopology, NormalizedPosition, Output, SamplingMode, SpatialLayout,
    StripDirection,
};
use uuid::Uuid;

// ── Helpers ─────────────────────────────────────────────────────────────

/// Build a minimal `TransitionSpec`.
fn transition_spec(duration_ms: u64, easing: EasingFunction) -> TransitionSpec {
    TransitionSpec {
        duration_ms,
        easing,
        color_interpolation: ColorInterpolation::Oklab,
    }
}

/// Build a simple automation rule.
fn make_rule(
    name: &str,
    trigger: TriggerSource,
    action: ActionKind,
    cooldown_secs: u64,
    enabled: bool,
) -> AutomationRule {
    AutomationRule {
        name: name.to_string(),
        trigger,
        conditions: Vec::new(),
        action,
        cooldown_secs,
        enabled,
    }
}

fn sample_layout(zone_id: &str) -> SpatialLayout {
    SpatialLayout {
        id: format!("layout-{zone_id}"),
        name: format!("Layout {zone_id}"),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones: vec![Output {
            id: zone_id.into(),
            name: zone_id.into(),
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

fn grouped_scene(name: &str, zone_id: &str, effect_id: EffectId) -> hypercolor_types::scene::Scene {
    let mut scene = make_scene(name);
    scene.zones = vec![Zone {
        id: ZoneId::new(),
        name: format!("{name} Group"),
        description: None,
        layers: vec![effect_layer(effect_id, 0.5)],
        layout: sample_layout(zone_id),
        brightness: 0.8,
        enabled: true,
        color: None,
        display_target: None,
        role: ZoneRole::Custom,
        controls_version: 0,
        layers_version: 0,
    }];
    scene.unassigned_behavior = UnassignedBehavior::Off;
    scene
}

fn color_layer(rgba: [f32; 4]) -> SceneLayer {
    SceneLayer {
        id: SceneLayerId::new(),
        name: None,
        source: LayerSource::ColorFill { rgba },
        blend: LayerBlendMode::Alpha,
        opacity: 1.0,
        transform: LayerTransform::default(),
        adjust: LayerAdjust::default(),
        bindings: Vec::new(),
        enabled: true,
    }
}

fn effect_layer(effect_id: EffectId, speed: f32) -> SceneLayer {
    SceneLayer::from_effect(
        SceneLayerId::new(),
        effect_id,
        HashMap::from([("speed".into(), ControlValue::Float(speed))]),
        HashMap::new(),
        None,
    )
}

fn zone_effect_id(zone: &Zone) -> Option<EffectId> {
    zone.layers.iter().find_map(|layer| match layer.source {
        LayerSource::Effect { effect_id, .. } => Some(effect_id),
        _ => None,
    })
}

fn zone_control<'a>(zone: &'a Zone, key: &str) -> Option<&'a ControlValue> {
    zone.layers.iter().find_map(|layer| match &layer.source {
        LayerSource::Effect { controls, .. } => controls.get(key),
        _ => None,
    })
}

fn effect_metadata(id: EffectId, name: &str) -> EffectMetadata {
    EffectMetadata {
        id,
        name: name.into(),
        author: "Hypercolor".into(),
        version: "1.0.0".into(),
        description: format!("{name} test effect"),
        category: EffectCategory::Ambient,
        tags: Vec::new(),
        controls: Vec::new(),
        presets: Vec::new(),
        audio_reactive: false,
        screen_reactive: false,
        input_reactive: false,
        source: EffectSource::Native {
            path: PathBuf::from(format!("native/{name}.rs")),
        },
        license: None,
    }
}

// ═══════════════════════════════════════════════════════════════════════
// SceneManager Tests
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn scene_manager_create_and_get() {
    let mut mgr = SceneManager::new();
    let scene = make_scene("Cozy Evening");
    let id = scene.id;

    mgr.create(scene).expect("create should succeed");

    let retrieved = mgr.get(&id).expect("scene should exist");
    assert_eq!(retrieved.name, "Cozy Evening");
}

#[test]
fn scene_manager_create_duplicate_fails() {
    let mut mgr = SceneManager::new();
    let scene = make_scene("Duplicate");
    let dupe = scene.clone();

    mgr.create(scene).expect("first create should succeed");
    let result = mgr.create(dupe);
    assert!(result.is_err(), "duplicate create should fail");
}

#[test]
fn scene_manager_create_rejects_overlapping_render_groups() {
    let mut mgr = SceneManager::new();
    let mut scene = make_scene("Grouped");
    scene.zones = vec![
        Zone {
            id: ZoneId::new(),
            name: "Desk".into(),
            description: None,
            layers: Vec::new(),
            layout: sample_layout("shared:zone"),
            brightness: 1.0,
            enabled: true,
            color: None,
            display_target: None,
            role: ZoneRole::Custom,
            controls_version: 0,
            layers_version: 0,
        },
        Zone {
            id: ZoneId::new(),
            name: "Room".into(),
            description: None,
            layers: Vec::new(),
            layout: sample_layout("shared:zone"),
            brightness: 1.0,
            enabled: true,
            color: None,
            display_target: None,
            role: ZoneRole::Custom,
            controls_version: 0,
            layers_version: 0,
        },
    ];

    let result = mgr.create(scene);
    assert!(result.is_err(), "overlapping zones should fail");
}

#[test]
fn scene_manager_list() {
    let mut mgr = SceneManager::new();
    mgr.create(make_scene("A")).expect("create A");
    mgr.create(make_scene("B")).expect("create B");
    mgr.create(make_scene("C")).expect("create C");

    let scenes = mgr.list();
    assert_eq!(scenes.len(), 3);
}

#[test]
fn scene_manager_update() {
    let mut mgr = SceneManager::new();
    let mut scene = make_scene("Original");
    let id = scene.id;
    mgr.create(scene.clone()).expect("create");

    scene.name = "Updated".to_string();
    mgr.update(scene).expect("update should succeed");

    let retrieved = mgr.get(&id).expect("scene should exist");
    assert_eq!(retrieved.name, "Updated");
}

#[test]
fn scene_manager_update_nonexistent_fails() {
    let mut mgr = SceneManager::new();
    let scene = make_scene("Ghost");
    let result = mgr.update(scene);
    assert!(result.is_err(), "update nonexistent should fail");
}

#[test]
fn scene_manager_delete() {
    let mut mgr = SceneManager::new();
    let scene = make_scene("Doomed");
    let id = scene.id;
    mgr.create(scene).expect("create");

    let deleted = mgr.delete(&id).expect("delete should succeed");
    assert_eq!(deleted.name, "Doomed");
    assert!(mgr.get(&id).is_none(), "scene should be gone");
    assert_eq!(mgr.scene_count(), 0);
}

#[test]
fn scene_manager_delete_nonexistent_fails() {
    let mut mgr = SceneManager::new();
    let id = SceneId::new();
    let result = mgr.delete(&id);
    assert!(result.is_err(), "delete nonexistent should fail");
}

#[test]
fn scene_manager_activate_and_active_tracking() {
    let mut mgr = SceneManager::new();
    let scene = make_scene("Active One");
    let id = scene.id;
    mgr.create(scene).expect("create");

    mgr.activate(&id, None).expect("activate should succeed");

    let active = mgr.active_scene_id().expect("should have active scene");
    assert_eq!(*active, id);
}

#[test]
fn scene_manager_caches_active_render_groups() {
    let mut mgr = SceneManager::new();
    let grouped = grouped_scene("Grouped", "desk:main", EffectId::from(Uuid::now_v7()));
    let grouped_id = grouped.id;
    let plain = make_scene("Plain");
    let plain_id = plain.id;

    mgr.create(grouped).expect("create grouped");
    mgr.create(plain).expect("create plain");

    assert!(mgr.active_render_groups().is_empty());
    assert_eq!(mgr.active_render_groups_revision(), 0);

    mgr.activate(&grouped_id, None).expect("activate grouped");
    assert_eq!(mgr.active_render_groups().len(), 1);
    let grouped_revision = mgr.active_render_groups_revision();
    assert!(grouped_revision > 0);

    mgr.activate(&plain_id, None).expect("activate plain");
    assert!(mgr.active_render_groups().is_empty());
    assert!(mgr.active_render_groups_revision() > grouped_revision);
}

#[test]
fn scene_manager_refreshes_active_render_group_cache_on_update() {
    let mut mgr = SceneManager::new();
    let mut scene = grouped_scene("Grouped", "desk:main", EffectId::from(Uuid::now_v7()));
    let id = scene.id;

    mgr.create(scene.clone()).expect("create grouped");
    mgr.activate(&id, None).expect("activate grouped");
    let initial_revision = mgr.active_render_groups_revision();
    assert_eq!(
        mgr.active_render_groups()[0].layout.zones[0].id,
        "desk:main"
    );

    scene.zones[0].layout = sample_layout("desk:updated");
    mgr.update(scene).expect("update grouped");

    assert_eq!(
        mgr.active_render_groups()[0].layout.zones[0].id,
        "desk:updated"
    );
    assert!(mgr.active_render_groups_revision() > initial_revision);
}

#[test]
fn scene_manager_upsert_primary_group_replaces_authored_layer_stack() {
    let mut mgr = SceneManager::new();
    let old_id = EffectId::from(Uuid::now_v7());
    let new_id = EffectId::from(Uuid::now_v7());
    let mut scene = grouped_scene("Primary", "desk:main", old_id);
    let scene_id = scene.id;
    scene.zones[0].role = ZoneRole::Primary;
    scene.zones[0].layers = vec![effect_layer(old_id, 0.25)];
    scene.zones[0].layers_version = 4;
    let previous_layer_id = scene.zones[0].layers[0].id;

    mgr.create(scene).expect("create primary scene");
    mgr.activate(&scene_id, None)
        .expect("activate primary scene");
    let initial_revision = mgr.active_render_groups_revision();

    let updated = mgr
        .upsert_primary_group(
            &effect_metadata(new_id, "plasma"),
            HashMap::from([("speed".into(), ControlValue::Float(1.0))]),
            None,
            sample_layout("desk:updated"),
        )
        .expect("upsert primary group")
        .clone();

    assert_eq!(zone_effect_id(&updated), Some(new_id));
    assert_eq!(updated.layers_version, 5);
    assert_eq!(updated.controls_version, 1);
    let [layer] = updated.layers.as_slice() else {
        panic!("apply should replace the stack with one effect layer");
    };
    assert_ne!(layer.id, previous_layer_id);
    let LayerSource::Effect {
        effect_id,
        controls,
        control_bindings,
        preset_id,
    } = &layer.source
    else {
        panic!("replacement layer should be an effect layer");
    };
    assert_eq!(*effect_id, new_id);
    assert_eq!(controls.get("speed"), Some(&ControlValue::Float(1.0)));
    assert!(control_bindings.is_empty());
    assert_eq!(*preset_id, None);

    let active_layers = mgr.active_render_groups()[0].layers.clone();
    let LayerSource::Effect { effect_id, .. } = active_layers[0].source else {
        panic!("active zone should expose the replacement effect layer");
    };
    assert_eq!(effect_id, new_id);
    assert!(mgr.active_render_groups_revision() > initial_revision);
}

#[test]
fn scene_manager_reapplying_an_effect_mints_another_layer_id() {
    let mut mgr = SceneManager::new();
    let effect_id = EffectId::from(Uuid::now_v7());
    let mut scene = grouped_scene("Primary", "desk:main", effect_id);
    let scene_id = scene.id;
    scene.zones[0].role = ZoneRole::Primary;

    mgr.create(scene).expect("create primary scene");
    mgr.activate(&scene_id, None)
        .expect("activate primary scene");

    let first = mgr
        .upsert_primary_group(
            &effect_metadata(effect_id, "plasma"),
            HashMap::new(),
            None,
            sample_layout("desk:first"),
        )
        .expect("first apply")
        .layers[0]
        .id;
    let second = mgr
        .upsert_primary_group(
            &effect_metadata(effect_id, "plasma"),
            HashMap::new(),
            None,
            sample_layout("desk:second"),
        )
        .expect("second apply")
        .layers[0]
        .id;

    assert_ne!(first, second);
}

#[test]
fn scene_manager_clear_group_effect_clears_effect_layers() {
    let mut mgr = SceneManager::new();
    let effect_id = EffectId::from(Uuid::now_v7());
    let mut scene = grouped_scene("Clearable", "desk:main", effect_id);
    let scene_id = scene.id;
    let group_id = scene.zones[0].id;
    scene.zones[0].layers = vec![effect_layer(effect_id, 0.5)];
    scene.zones[0].layers_version = 2;

    mgr.create(scene).expect("create grouped scene");
    mgr.activate(&scene_id, None)
        .expect("activate grouped scene");

    let updated = mgr
        .clear_group_effect(group_id)
        .expect("clear group effect")
        .clone();

    assert_eq!(zone_effect_id(&updated), None);
    assert!(updated.layers.is_empty());
    assert!(updated.layers.clone().is_empty());
    assert_eq!(updated.layers_version, 3);
    assert!(mgr.active_render_groups()[0].layers.clone().is_empty());
}

#[test]
fn scene_manager_reset_group_controls_updates_effect_layer() {
    let mut mgr = SceneManager::new();
    let effect_id = EffectId::from(Uuid::now_v7());
    let mut scene = grouped_scene("Reset", "desk:main", effect_id);
    let scene_id = scene.id;
    let group_id = scene.zones[0].id;
    scene.zones[0].layers = vec![
        color_layer([0.0, 0.0, 0.0, 1.0]),
        effect_layer(effect_id, 0.5),
    ];
    scene.zones[0].layers_version = 7;

    mgr.create(scene).expect("create grouped scene");
    mgr.activate(&scene_id, None)
        .expect("activate grouped scene");

    let updated = mgr
        .reset_group_controls(
            group_id,
            HashMap::from([("speed".into(), ControlValue::Float(1.75))]),
        )
        .expect("reset group controls")
        .clone();

    assert_eq!(
        zone_control(&updated, "speed"),
        Some(&ControlValue::Float(1.75))
    );
    assert_eq!(updated.controls_version, 1);
    assert_eq!(updated.layers_version, 8);

    let layer = updated
        .layers
        .iter()
        .find(|layer| matches!(layer.source, LayerSource::Effect { .. }))
        .expect("effect layer should remain");
    let LayerSource::Effect { controls, .. } = &layer.source else {
        panic!("target layer should remain an effect layer");
    };
    assert_eq!(controls.get("speed"), Some(&ControlValue::Float(1.75)));
}

#[test]
fn scene_manager_add_layer_preserves_authored_effect_and_refreshes_cache() {
    let mut mgr = SceneManager::new();
    let effect_id = EffectId::from(Uuid::now_v7());
    let scene = grouped_scene("Layered", "desk:main", effect_id);
    let scene_id = scene.id;
    let group_id = scene.zones[0].id;

    mgr.create(scene).expect("create grouped scene");
    let authored_layer_id = mgr
        .get(&scene_id)
        .and_then(|scene| scene.zones.first())
        .and_then(|zone| zone.layers.first())
        .map(|layer| layer.id)
        .expect("grouped scene should retain its authored effect layer");
    mgr.activate(&scene_id, None)
        .expect("activate grouped scene");
    let initial_revision = mgr.active_render_groups_revision();

    let overlay = color_layer([1.0, 0.0, 0.5, 1.0]);
    let overlay_id = overlay.id;
    let (updated, version) = mgr
        .add_group_layer(group_id, overlay, Some(0))
        .expect("add layer");
    let updated = updated.clone();

    assert_eq!(version, 1);
    assert_eq!(updated.layers_version, 1);
    assert_eq!(updated.layers.len(), 2);
    assert_eq!(updated.layers[0].id, authored_layer_id);
    assert_ne!(updated.layers[0].id.as_uuid(), group_id.0);
    assert_eq!(updated.layers[1].id, overlay_id);
    assert_eq!(zone_effect_id(&updated), Some(effect_id));
    assert!(mgr.active_render_groups_revision() > initial_revision);
    assert_eq!(mgr.active_render_groups()[0].layers[1].id, overlay_id);
}

#[test]
fn scene_manager_update_and_remove_layers_bump_versions() {
    let mut mgr = SceneManager::new();
    let effect_id = EffectId::from(Uuid::now_v7());
    let mut scene = grouped_scene("Mutable", "desk:main", effect_id);
    let scene_id = scene.id;
    let group_id = scene.zones[0].id;
    let base = effect_layer(effect_id, 0.5);
    let mut overlay = color_layer([0.0, 0.25, 1.0, 1.0]);
    let overlay_id = overlay.id;
    overlay.opacity = 0.75;
    scene.zones[0].layers = vec![base, overlay.clone()];

    mgr.create(scene).expect("create grouped scene");
    mgr.activate(&scene_id, None)
        .expect("activate grouped scene");

    overlay.opacity = 0.25;
    let (updated, update_version) = mgr
        .update_group_layer(group_id, overlay_id, overlay, Some(0))
        .expect("update layer");
    assert_eq!(update_version, 1);
    assert_eq!(updated.layers[1].opacity, 0.25);

    let (updated, remove_version) = mgr
        .remove_group_layer(group_id, overlay_id, Some(1))
        .expect("remove layer");
    assert_eq!(remove_version, 2);
    assert_eq!(updated.layers.len(), 1);

    let missing = mgr
        .remove_group_layer(group_id, overlay_id, Some(2))
        .expect_err("removed layer should be missing");
    assert_eq!(
        missing,
        LayerMutationError::LayerMissing {
            layer_id: overlay_id
        }
    );
}

#[test]
fn scene_manager_reorder_layers_requires_exact_permutation() {
    let mut mgr = SceneManager::new();
    let effect_id = EffectId::from(Uuid::now_v7());
    let mut scene = grouped_scene("Ordered", "desk:main", effect_id);
    let scene_id = scene.id;
    let group_id = scene.zones[0].id;
    let base = color_layer([0.0, 0.0, 0.0, 1.0]);
    let top = effect_layer(effect_id, 0.5);
    let base_id = base.id;
    let top_id = top.id;
    scene.zones[0].layers = vec![base, top];

    mgr.create(scene).expect("create grouped scene");
    mgr.activate(&scene_id, None)
        .expect("activate grouped scene");

    let invalid = mgr
        .reorder_group_layers(group_id, vec![top_id], Some(0))
        .expect_err("missing layer id should reject");
    assert_eq!(invalid, LayerMutationError::InvalidOrder);

    let (updated, version) = mgr
        .reorder_group_layers(group_id, vec![top_id, base_id], Some(0))
        .expect("reorder layers");
    let updated = updated.clone();
    assert_eq!(version, 1);
    assert_eq!(updated.layers[0].id, top_id);
    assert_eq!(updated.layers[1].id, base_id);

    let stale = mgr
        .reorder_group_layers(group_id, vec![base_id, top_id], Some(0))
        .expect_err("stale reorder should fail");
    assert_eq!(
        stale,
        LayerMutationError::Stale {
            expected: 0,
            current: 1
        }
    );
}

#[test]
fn scene_manager_patch_layer_effect_controls_uses_layers_version() {
    let mut mgr = SceneManager::new();
    let effect_id = EffectId::from(Uuid::now_v7());
    let mut scene = grouped_scene("Controls", "desk:main", effect_id);
    let scene_id = scene.id;
    let group_id = scene.zones[0].id;
    let base = color_layer([0.0, 0.0, 0.0, 1.0]);
    let effect = effect_layer(effect_id, 0.5);
    let layer_id = effect.id;
    scene.zones[0].layers = vec![base, effect];

    mgr.create(scene).expect("create grouped scene");
    mgr.activate(&scene_id, None)
        .expect("activate grouped scene");

    let (updated, version) = mgr
        .patch_layer_effect_controls(
            group_id,
            layer_id,
            HashMap::from([("speed".into(), ControlValue::Float(1.25))]),
            Some(0),
        )
        .expect("patch layer controls");
    let updated = updated.clone();

    assert_eq!(version, 1);
    assert_eq!(updated.layers_version, 1);
    assert_eq!(updated.controls_version, 0);
    assert_eq!(
        zone_control(&updated, "speed"),
        Some(&ControlValue::Float(1.25))
    );
    let layer = updated
        .layers
        .iter()
        .find(|layer| layer.id == layer_id)
        .expect("effect layer should remain");
    let LayerSource::Effect { controls, .. } = &layer.source else {
        panic!("target layer should remain an effect layer");
    };
    assert_eq!(controls.get("speed"), Some(&ControlValue::Float(1.25)));

    let stale = mgr
        .patch_layer_effect_controls(
            group_id,
            layer_id,
            HashMap::from([("speed".into(), ControlValue::Float(2.0))]),
            Some(0),
        )
        .expect_err("stale control patch should fail");
    assert_eq!(
        stale,
        LayerMutationError::Stale {
            expected: 0,
            current: 1
        }
    );
}

#[test]
fn scene_manager_activate_nonexistent_fails() {
    let mut mgr = SceneManager::new();
    let id = SceneId::new();
    let result = mgr.activate(&id, None);
    assert!(result.is_err(), "activate nonexistent should fail");
}

#[test]
fn scene_manager_deactivate_restores_previous() {
    let mut mgr = SceneManager::new();

    let scene_a = make_scene("Base");
    let id_a = scene_a.id;
    mgr.create(scene_a).expect("create A");

    let mut scene_b = make_scene("Overlay");
    scene_b.priority = ScenePriority::TRIGGER;
    let id_b = scene_b.id;
    mgr.create(scene_b).expect("create B");

    // Activate base first, then overlay.
    mgr.activate(&id_a, None).expect("activate A");
    mgr.activate(&id_b, None).expect("activate B");

    // B should be active (higher priority).
    assert_eq!(*mgr.active_scene_id().expect("active"), id_b);

    // Deactivate current (B) — A should restore.
    mgr.deactivate_current();
    assert_eq!(*mgr.active_scene_id().expect("active"), id_a);
}

#[test]
fn scene_manager_deactivate_empty_is_noop() {
    let mut mgr = SceneManager::new();
    // Should not panic.
    mgr.deactivate_current();
    assert!(mgr.active_scene_id().is_none());
}

#[test]
fn scene_manager_transition_plan_keeps_authored_zones_without_flat_assignments() {
    let mut mgr = SceneManager::new();
    let scene_a = grouped_scene("Ambient", "desk:main", EffectId::from(Uuid::now_v7()));
    let scene_b = grouped_scene("Focus", "desk:main", EffectId::from(Uuid::now_v7()));
    let id_a = scene_a.id;
    let id_b = scene_b.id;
    let zone_b = scene_b.zones[0].clone();

    mgr.create(scene_a).expect("create scene A");
    mgr.create(scene_b).expect("create scene B");
    mgr.activate(&id_a, None).expect("activate A");
    mgr.activate(&id_b, None).expect("activate B");

    let transition = mgr.transition_plan().expect("transition should exist");
    assert_eq!(transition.from_scene, id_a);
    assert_eq!(transition.to_scene, id_b);

    let plan = mgr.plan_snapshot(7);
    assert_eq!(plan.generation, 7);
    assert_eq!(plan.zones.as_ref(), &[zone_b]);
    assert!(plan.transition.is_some());
}

// ═══════════════════════════════════════════════════════════════════════
// Transition Tests
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn transition_color_interpolation_oklab() {
    // Verify the helper produces a valid intermediate color.
    let red = LinearRgba::new(1.0, 0.0, 0.0, 1.0);
    let blue = LinearRgba::new(0.0, 0.0, 1.0, 1.0);

    let mid = hypercolor_core::scene::interpolate_oklab(&red, &blue, 0.5);

    // The midpoint in Oklab should NOT be a muddy gray (which sRGB lerp produces).
    // It should be a vivid purple-ish tone. Alpha should be 1.0.
    assert!((mid.a - 1.0).abs() < f32::EPSILON, "alpha should be 1.0");
    // The result should have some chroma (not gray).
    let oklab = mid.to_oklab();
    let chroma = (oklab.a * oklab.a + oklab.b * oklab.b).sqrt();
    assert!(
        chroma > 0.05,
        "Oklab midpoint should have visible chroma, got {chroma}"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// PriorityStack Tests
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn priority_stack_push_peek_returns_highest() {
    let mut stack = PriorityStack::new();

    let low_id = SceneId::new();
    let high_id = SceneId::new();

    stack.push(low_id, ScenePriority::AMBIENT);
    stack.push(high_id, ScenePriority::ALERT);

    let top = stack.peek().expect("stack should not be empty");
    assert_eq!(top.scene_id, high_id);
    assert_eq!(top.priority, ScenePriority::ALERT);
}

#[test]
fn priority_stack_pop_restores_next_highest() {
    let mut stack = PriorityStack::new();

    let base_id = SceneId::new();
    let overlay_id = SceneId::new();

    stack.push(base_id, ScenePriority::AMBIENT);
    stack.push(overlay_id, ScenePriority::USER);

    // Pop the overlay.
    let popped = stack.pop().expect("pop should succeed");
    assert_eq!(popped.scene_id, overlay_id);

    // Base should now be on top.
    let top = stack.peek().expect("base should remain");
    assert_eq!(top.scene_id, base_id);
}

#[test]
fn priority_stack_equal_priority_fifo() {
    let mut stack = PriorityStack::new();

    let first_id = SceneId::new();
    let second_id = SceneId::new();

    stack.push(first_id, ScenePriority::USER);
    // Small sleep to ensure distinct timestamps for FIFO ordering.
    thread::sleep(Duration::from_millis(2));
    stack.push(second_id, ScenePriority::USER);

    // The most recently pushed entry should win (FIFO: last-in is active).
    let top = stack.peek().expect("stack should not be empty");
    assert_eq!(
        top.scene_id, second_id,
        "most recently pushed equal-priority entry should win"
    );
}

#[test]
fn priority_stack_empty_returns_none() {
    let stack = PriorityStack::new();
    assert!(stack.peek().is_none());
    assert!(stack.is_empty());
    assert_eq!(stack.len(), 0);
}

#[test]
fn priority_stack_remove_by_id() {
    let mut stack = PriorityStack::new();

    let a = SceneId::new();
    let b = SceneId::new();
    let c = SceneId::new();

    stack.push(a, ScenePriority::AMBIENT);
    stack.push(b, ScenePriority::USER);
    stack.push(c, ScenePriority::TRIGGER);

    // Remove the middle entry.
    assert!(stack.remove(&b));
    assert_eq!(stack.len(), 2);

    // Top should still be the highest priority.
    let top = stack.peek().expect("stack should not be empty");
    assert_eq!(top.scene_id, c);
}

#[test]
fn priority_stack_pop_empty_returns_none() {
    let mut stack = PriorityStack::new();
    assert!(stack.pop().is_none());
}

#[test]
fn priority_stack_multiple_priorities_order() {
    let mut stack = PriorityStack::new();

    let ambient_id = SceneId::new();
    let user_id = SceneId::new();
    let trigger_id = SceneId::new();
    let alert_id = SceneId::new();

    // Push in arbitrary order.
    stack.push(user_id, ScenePriority::USER);
    stack.push(alert_id, ScenePriority::ALERT);
    stack.push(ambient_id, ScenePriority::AMBIENT);
    stack.push(trigger_id, ScenePriority::TRIGGER);

    // Top should be alert (highest).
    let entries = stack.entries();
    assert_eq!(entries.len(), 4);
    assert_eq!(entries[0].scene_id, alert_id);
    assert_eq!(entries[1].scene_id, trigger_id);
    assert_eq!(entries[2].scene_id, user_id);
    assert_eq!(entries[3].scene_id, ambient_id);
}

// ═══════════════════════════════════════════════════════════════════════
// AutomationEngine Tests
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn automation_add_remove_rules() {
    let mut engine = AutomationEngine::new();

    let rule = make_rule(
        "sunset-cozy",
        TriggerSource::Sunset,
        ActionKind::ActivateScene("cozy".to_string()),
        0,
        true,
    );

    engine.add_rule(rule);
    assert_eq!(engine.rule_count(), 1);
    assert!(engine.get_rule("sunset-cozy").is_some());

    let removed = engine.remove_rule("sunset-cozy");
    assert!(removed.is_some());
    assert_eq!(engine.rule_count(), 0);
}

#[test]
fn automation_enable_disable_rules() {
    let mut engine = AutomationEngine::new();

    let rule = make_rule(
        "game-mode",
        TriggerSource::GameDetected,
        ActionKind::ActivateScene("gaming".to_string()),
        0,
        true,
    );
    engine.add_rule(rule);

    // Disable the rule.
    assert!(engine.disable_rule("game-mode"));
    let r = engine.get_rule("game-mode").expect("rule should exist");
    assert!(!r.enabled);

    // Re-enable.
    assert!(engine.enable_rule("game-mode"));
    let r = engine.get_rule("game-mode").expect("rule should exist");
    assert!(r.enabled);
}

#[test]
fn automation_enable_nonexistent_returns_false() {
    let mut engine = AutomationEngine::new();
    assert!(!engine.enable_rule("phantom"));
    assert!(!engine.disable_rule("phantom"));
}

#[test]
fn automation_evaluate_triggers_fire_matching() {
    let mut engine = AutomationEngine::new();

    engine.add_rule(make_rule(
        "sunset-rule",
        TriggerSource::Sunset,
        ActionKind::ActivateScene("evening".to_string()),
        0,
        true,
    ));

    engine.add_rule(make_rule(
        "sunrise-rule",
        TriggerSource::Sunrise,
        ActionKind::ActivateScene("morning".to_string()),
        0,
        true,
    ));

    // Fire a sunset trigger — only the sunset rule should match.
    let results = engine.evaluate(&TriggerSource::Sunset);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].0, "sunset-rule");

    // Fire a sunrise trigger.
    let results = engine.evaluate(&TriggerSource::Sunrise);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].0, "sunrise-rule");
}

#[test]
fn automation_evaluate_no_match() {
    let mut engine = AutomationEngine::new();

    engine.add_rule(make_rule(
        "sunset-only",
        TriggerSource::Sunset,
        ActionKind::ActivateScene("evening".to_string()),
        0,
        true,
    ));

    let results = engine.evaluate(&TriggerSource::Manual);
    assert!(results.is_empty(), "no rules should match Manual trigger");
}

#[test]
fn automation_cooldown_prevents_rapid_firing() {
    let mut engine = AutomationEngine::new();

    engine.add_rule(make_rule(
        "game-alert",
        TriggerSource::GameDetected,
        ActionKind::ActivateScene("gaming".to_string()),
        5, // 5 second cooldown
        true,
    ));

    // First evaluation should fire.
    let results = engine.evaluate(&TriggerSource::GameDetected);
    assert_eq!(results.len(), 1, "first trigger should fire");

    // Immediate second evaluation should be blocked by cooldown.
    let results = engine.evaluate(&TriggerSource::GameDetected);
    assert!(results.is_empty(), "cooldown should prevent re-firing");
}

#[test]
fn automation_cooldown_resets() {
    let mut engine = AutomationEngine::new();

    engine.add_rule(make_rule(
        "rapid",
        TriggerSource::Manual,
        ActionKind::RestorePrevious,
        1, // 1 second cooldown
        true,
    ));

    // Fire once.
    let results = engine.evaluate(&TriggerSource::Manual);
    assert_eq!(results.len(), 1);

    // Wait for cooldown to expire.
    thread::sleep(Duration::from_millis(1100));

    // Should fire again.
    let results = engine.evaluate(&TriggerSource::Manual);
    assert_eq!(results.len(), 1, "rule should fire after cooldown expires");
}

#[test]
fn automation_disabled_rules_dont_fire() {
    let mut engine = AutomationEngine::new();

    engine.add_rule(make_rule(
        "disabled-rule",
        TriggerSource::Manual,
        ActionKind::ActivateScene("test".to_string()),
        0,
        false, // disabled
    ));

    let results = engine.evaluate(&TriggerSource::Manual);
    assert!(results.is_empty(), "disabled rule should not fire");
}

#[test]
fn automation_app_launched_matching() {
    let mut engine = AutomationEngine::new();

    engine.add_rule(make_rule(
        "vscode-focus",
        TriggerSource::AppLaunched("code".to_string()),
        ActionKind::ActivateScene("coding".to_string()),
        0,
        true,
    ));

    // Matching app.
    let results = engine.evaluate(&TriggerSource::AppLaunched("code".to_string()));
    assert_eq!(results.len(), 1);

    // Non-matching app.
    let results = engine.evaluate(&TriggerSource::AppLaunched("firefox".to_string()));
    assert!(results.is_empty());
}

#[test]
fn automation_time_of_day_matching() {
    let mut engine = AutomationEngine::new();

    engine.add_rule(make_rule(
        "nine-am",
        TriggerSource::TimeOfDay { hour: 9, minute: 0 },
        ActionKind::SetBrightness(1.0),
        0,
        true,
    ));

    // Matching time.
    let results = engine.evaluate(&TriggerSource::TimeOfDay { hour: 9, minute: 0 });
    assert_eq!(results.len(), 1);

    // Non-matching time.
    let results = engine.evaluate(&TriggerSource::TimeOfDay {
        hour: 10,
        minute: 0,
    });
    assert!(results.is_empty());
}

#[test]
fn automation_conditions_evaluated() {
    let mut engine = AutomationEngine::new();

    let mut rule = make_rule(
        "conditional",
        TriggerSource::Manual,
        ActionKind::RestorePrevious,
        0,
        true,
    );
    // Add a condition that evaluates to false.
    rule.conditions = vec!["false".to_string()];
    engine.add_rule(rule);

    let results = engine.evaluate(&TriggerSource::Manual);
    assert!(results.is_empty(), "false condition should block firing");
}

#[test]
fn automation_true_condition_passes() {
    let mut engine = AutomationEngine::new();

    let mut rule = make_rule(
        "truthy",
        TriggerSource::Manual,
        ActionKind::RestorePrevious,
        0,
        true,
    );
    rule.conditions = vec!["true".to_string()];
    engine.add_rule(rule);

    let results = engine.evaluate(&TriggerSource::Manual);
    assert_eq!(results.len(), 1, "true condition should allow firing");
}

#[test]
fn automation_reset_cooldown() {
    let mut engine = AutomationEngine::new();

    engine.add_rule(make_rule(
        "resettable",
        TriggerSource::Manual,
        ActionKind::RestorePrevious,
        60, // long cooldown
        true,
    ));

    // Fire once.
    let results = engine.evaluate(&TriggerSource::Manual);
    assert_eq!(results.len(), 1);

    // Blocked by cooldown.
    let results = engine.evaluate(&TriggerSource::Manual);
    assert!(results.is_empty());

    // Reset cooldown.
    engine.reset_cooldown("resettable");

    // Should fire again.
    let results = engine.evaluate(&TriggerSource::Manual);
    assert_eq!(results.len(), 1, "reset cooldown should allow re-firing");
}

// ═══════════════════════════════════════════════════════════════════════
// Integration: SceneManager with Transitions
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn scene_manager_activate_starts_transition() {
    let mut mgr = SceneManager::new();

    let scene_a = make_scene("Scene A");
    let id_a = scene_a.id;
    mgr.create(scene_a).expect("create A");

    let scene_b = make_scene("Scene B");
    let id_b = scene_b.id;
    mgr.create(scene_b).expect("create B");

    // Activate A — no transition (first activation).
    mgr.activate(&id_a, None).expect("activate A");
    assert!(mgr.transition_plan().is_none());

    // Activate B — should start a transition from A to B.
    mgr.activate(&id_b, Some(transition_spec(500, EasingFunction::Linear)))
        .expect("activate B");
    let plan = mgr.transition_plan().expect("transition plan should exist");
    assert_eq!(plan.from_scene, id_a);
    assert_eq!(plan.to_scene, id_b);
    assert_eq!(plan.spec.duration_ms, 500);
}
