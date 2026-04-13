//! Tests for the scene engine: manager, transitions, priority stack, and automation.

use std::collections::HashMap;
use std::thread;
use std::time::Duration;

use hypercolor_core::scene::automation::AutomationEngine;
use hypercolor_core::scene::priority::PriorityStack;
use hypercolor_core::scene::transition::TransitionState;
use hypercolor_core::scene::{SceneManager, make_scene};
use hypercolor_types::canvas::RgbaF32;
use hypercolor_types::effect::{ControlValue, EffectId};
use hypercolor_types::scene::{
    ActionKind, AutomationRule, ColorInterpolation, EasingFunction, RenderGroup, RenderGroupId,
    RenderGroupRole, SceneId, ScenePriority, TransitionSpec, TriggerSource, UnassignedBehavior,
    ZoneAssignment,
};
use hypercolor_types::spatial::{
    DeviceZone, EdgeBehavior, LedTopology, NormalizedPosition, SamplingMode, SpatialLayout,
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

/// Build a zone assignment.
fn zone(name: &str, effect: &str, brightness: Option<f32>) -> ZoneAssignment {
    ZoneAssignment {
        zone_name: name.to_string(),
        effect_name: effect.to_string(),
        parameters: HashMap::new(),
        brightness,
    }
}

fn sample_layout(zone_id: &str) -> SpatialLayout {
    SpatialLayout {
        id: format!("layout-{zone_id}"),
        name: format!("Layout {zone_id}"),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones: vec![DeviceZone {
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
        }],
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    }
}

fn grouped_scene(name: &str, zone_id: &str, effect_id: EffectId) -> hypercolor_types::scene::Scene {
    let mut scene = make_scene(name);
    scene.groups = vec![RenderGroup {
        id: RenderGroupId::new(),
        name: format!("{name} Group"),
        description: None,
        effect_id: Some(effect_id),
        controls: HashMap::from([("speed".into(), ControlValue::Float(0.5))]),
        control_bindings: HashMap::new(),
        preset_id: None,
        layout: sample_layout(zone_id),
        brightness: 0.8,
        enabled: true,
        color: None,
        display_target: None,
        role: RenderGroupRole::Custom,
    }];
    scene.unassigned_behavior = UnassignedBehavior::Off;
    scene
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
    scene.groups = vec![
        RenderGroup {
            id: RenderGroupId::new(),
            name: "Desk".into(),
            description: None,
            effect_id: Some(EffectId::from(Uuid::now_v7())),
            controls: HashMap::new(),
            control_bindings: HashMap::new(),
            preset_id: None,
            layout: sample_layout("shared:zone"),
            brightness: 1.0,
            enabled: true,
            color: None,
            display_target: None,
            role: RenderGroupRole::Custom,
        },
        RenderGroup {
            id: RenderGroupId::new(),
            name: "Room".into(),
            description: None,
            effect_id: Some(EffectId::from(Uuid::now_v7())),
            controls: HashMap::new(),
            control_bindings: HashMap::new(),
            preset_id: None,
            layout: sample_layout("shared:zone"),
            brightness: 1.0,
            enabled: true,
            color: None,
            display_target: None,
            role: RenderGroupRole::Custom,
        },
    ];

    let result = mgr.create(scene);
    assert!(result.is_err(), "overlapping render groups should fail");
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

    scene.groups[0].layout = sample_layout("desk:updated");
    mgr.update(scene).expect("update grouped");

    assert_eq!(
        mgr.active_render_groups()[0].layout.zones[0].id,
        "desk:updated"
    );
    assert!(mgr.active_render_groups_revision() > initial_revision);
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
fn scene_manager_transition_uses_grouped_scene_assignments() {
    let mut mgr = SceneManager::new();
    let scene_a = grouped_scene("Ambient", "desk:main", EffectId::from(Uuid::now_v7()));
    let scene_b = grouped_scene("Focus", "desk:main", EffectId::from(Uuid::now_v7()));
    let id_a = scene_a.id;
    let id_b = scene_b.id;
    let effect_b = scene_b.groups[0]
        .effect_id
        .expect("grouped scene should carry an effect");

    mgr.create(scene_a).expect("create scene A");
    mgr.create(scene_b).expect("create scene B");
    mgr.activate(&id_a, None).expect("activate A");
    mgr.activate(&id_b, None).expect("activate B");

    let transition = mgr.active_transition().expect("transition should exist");
    let blended = transition.blend();
    assert_eq!(blended.len(), 1);
    assert_eq!(blended[0].zone_name, "desk:main");

    mgr.tick_transition(0.6);
    let blended = mgr
        .active_transition()
        .expect("transition should still exist")
        .blend();
    assert_eq!(blended[0].effect_name, effect_b.to_string());
}

// ═══════════════════════════════════════════════════════════════════════
// Transition Tests
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn transition_linear_progress() {
    let from = vec![zone("strip", "rainbow", Some(1.0))];
    let to = vec![zone("strip", "breathe", Some(0.5))];

    let mut transition = TransitionState::new(
        SceneId::new(),
        SceneId::new(),
        transition_spec(1000, EasingFunction::Linear),
        from,
        to,
    );

    assert!(!transition.is_complete());
    assert!((transition.progress - 0.0).abs() < f32::EPSILON);

    // Advance 500ms (half duration).
    transition.tick(0.5);
    assert!(
        (transition.progress - 0.5).abs() < 0.01,
        "progress should be ~0.5, got {}",
        transition.progress
    );

    // Advance another 500ms (complete).
    transition.tick(0.5);
    assert!(transition.is_complete());
    assert!((transition.progress - 1.0).abs() < f32::EPSILON);
}

#[test]
fn transition_easing_ease_in_slow_start() {
    let from = vec![zone("z", "static", Some(1.0))];
    let to = vec![zone("z", "wave", Some(1.0))];

    let mut transition = TransitionState::new(
        SceneId::new(),
        SceneId::new(),
        transition_spec(1000, EasingFunction::EaseIn),
        from,
        to,
    );

    // At 25% linear progress, eased progress should be less than 0.25
    // because EaseIn (cubic) starts slow: t^3 at 0.25 = 0.015625.
    transition.tick(0.25);
    let eased = transition.eased_progress();
    assert!(
        eased < 0.25,
        "EaseIn at 25% linear should produce eased < 0.25, got {eased}"
    );
    assert!(
        (eased - 0.015_625).abs() < 0.01,
        "EaseIn at 0.25 should be ~0.015625, got {eased}"
    );
}

#[test]
fn transition_completion_detection() {
    let mut transition = TransitionState::new(
        SceneId::new(),
        SceneId::new(),
        transition_spec(500, EasingFunction::Linear),
        vec![],
        vec![],
    );

    assert!(!transition.is_complete());

    // Overshoot.
    transition.tick(1.0);
    assert!(transition.is_complete());
    // Progress clamped to 1.0.
    assert!((transition.progress - 1.0).abs() < f32::EPSILON);

    // Further ticks are no-ops.
    transition.tick(1.0);
    assert!(transition.is_complete());
}

#[test]
fn transition_zero_duration_is_instant() {
    let transition = TransitionState::new(
        SceneId::new(),
        SceneId::new(),
        transition_spec(0, EasingFunction::Linear),
        vec![zone("z", "a", Some(1.0))],
        vec![zone("z", "b", Some(0.5))],
    );

    assert!(
        transition.is_complete(),
        "zero-duration transition should be instantly complete"
    );

    // Blend should return the target state.
    let blended = transition.blend();
    assert_eq!(blended.len(), 1);
    let b = blended.first().expect("should have one zone");
    // At t=1.0 the brightness should be the target's brightness.
    assert!(
        (b.brightness.unwrap_or(0.0) - 0.5).abs() < 0.01,
        "brightness should be ~0.5 (target), got {:?}",
        b.brightness
    );
}

#[test]
fn transition_blends_brightness() {
    let from = vec![zone("strip", "static", Some(1.0))];
    let to = vec![zone("strip", "static", Some(0.0))];

    let mut transition = TransitionState::new(
        SceneId::new(),
        SceneId::new(),
        transition_spec(1000, EasingFunction::Linear),
        from,
        to,
    );

    transition.tick(0.5);
    let blended = transition.blend();
    let b = blended.first().expect("one zone");
    let brightness = b.brightness.unwrap_or(0.0);
    assert!(
        (brightness - 0.5).abs() < 0.05,
        "midpoint brightness should be ~0.5, got {brightness}"
    );
}

#[test]
fn transition_color_interpolation_oklab() {
    // Verify the helper produces a valid intermediate color.
    let red = RgbaF32::new(1.0, 0.0, 0.0, 1.0);
    let blue = RgbaF32::new(0.0, 0.0, 1.0, 1.0);

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
    assert!(!mgr.is_transitioning());

    // Activate B — should start a transition from A to B.
    mgr.activate(&id_b, Some(transition_spec(500, EasingFunction::Linear)))
        .expect("activate B");
    assert!(mgr.is_transitioning());

    // Tick to completion.
    mgr.tick_transition(1.0);
    assert!(
        !mgr.is_transitioning(),
        "transition should complete after sufficient tick"
    );
}

#[test]
fn scene_manager_tick_transition_clears_on_complete() {
    let mut mgr = SceneManager::new();

    let a = make_scene("A");
    let id_a = a.id;
    mgr.create(a).expect("create A");

    let b = make_scene("B");
    let id_b = b.id;
    mgr.create(b).expect("create B");

    mgr.activate(&id_a, None).expect("activate A");
    mgr.activate(&id_b, Some(transition_spec(100, EasingFunction::Linear)))
        .expect("activate B");

    assert!(mgr.is_transitioning());

    // Small tick — not enough to complete.
    mgr.tick_transition(0.05);
    assert!(mgr.is_transitioning());

    // Large tick — should complete.
    mgr.tick_transition(1.0);
    assert!(!mgr.is_transitioning());
}
