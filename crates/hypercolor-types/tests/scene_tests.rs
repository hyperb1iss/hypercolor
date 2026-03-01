use hypercolor_types::scene::{
    ActionKind, AutomationRule, ColorInterpolation, EasingFunction, Scene, SceneId, ScenePriority,
    SceneScope, TransitionSpec, TriggerSource, ZoneAssignment,
};
use std::collections::HashMap;

// ── Helpers ──────────────────────────────────────────────────────────────

fn sample_transition() -> TransitionSpec {
    TransitionSpec {
        duration_ms: 1000,
        easing: EasingFunction::EaseInOut,
        color_interpolation: ColorInterpolation::Oklab,
    }
}

fn sample_scene() -> Scene {
    Scene {
        id: SceneId::new(),
        name: "Test Scene".into(),
        description: Some("A scene for testing".into()),
        scope: SceneScope::Full,
        zone_assignments: vec![ZoneAssignment {
            zone_name: "keyboard:main".into(),
            effect_name: "rainbow_wave".into(),
            parameters: HashMap::from([("speed".into(), "0.5".into())]),
            brightness: Some(0.8),
        }],
        transition: sample_transition(),
        priority: ScenePriority::USER,
        enabled: true,
        metadata: HashMap::from([("author".into(), "test".into())]),
    }
}

// ── SceneId ──────────────────────────────────────────────────────────────

#[test]
fn scene_id_new_creates_unique_ids() {
    let a = SceneId::new();
    let b = SceneId::new();
    assert_ne!(a, b);
}

#[test]
fn scene_id_default_creates_unique_ids() {
    let a = SceneId::default();
    let b = SceneId::default();
    assert_ne!(a, b);
}

#[test]
fn scene_id_clone_is_equal() {
    let id = SceneId::new();
    let cloned = id;
    assert_eq!(id, cloned);
}

#[test]
fn scene_id_display_matches_uuid() {
    let id = SceneId::new();
    assert_eq!(id.to_string(), id.0.to_string());
}

#[test]
fn scene_id_hash_works_in_collections() {
    let id = SceneId::new();
    let mut map = HashMap::new();
    map.insert(id, "scene_a");
    assert_eq!(map.get(&id), Some(&"scene_a"));
}

// ── Scene ────────────────────────────────────────────────────────────────

#[test]
fn scene_construction() {
    let scene = sample_scene();
    assert_eq!(scene.name, "Test Scene");
    assert_eq!(scene.description.as_deref(), Some("A scene for testing"));
    assert!(scene.enabled);
    assert_eq!(scene.priority, ScenePriority::USER);
    assert_eq!(scene.zone_assignments.len(), 1);
}

#[test]
fn scene_with_no_assignments() {
    let scene = Scene {
        id: SceneId::new(),
        name: "Empty".into(),
        description: None,
        scope: SceneScope::Full,
        zone_assignments: vec![],
        transition: sample_transition(),
        priority: ScenePriority::AMBIENT,
        enabled: false,
        metadata: HashMap::new(),
    };
    assert!(scene.zone_assignments.is_empty());
    assert!(!scene.enabled);
    assert!(scene.description.is_none());
}

#[test]
fn scene_json_round_trip() {
    let original = sample_scene();
    let json = serde_json::to_string(&original).expect("serialize Scene");
    let restored: Scene = serde_json::from_str(&json).expect("deserialize Scene");
    assert_eq!(restored.name, original.name);
    assert_eq!(restored.description, original.description);
    assert_eq!(restored.enabled, original.enabled);
    assert_eq!(
        restored.zone_assignments.len(),
        original.zone_assignments.len()
    );
}

// ── SceneScope ───────────────────────────────────────────────────────────

#[test]
fn scope_full_json_round_trip() {
    let scope = SceneScope::Full;
    let json = serde_json::to_string(&scope).expect("serialize");
    let restored: SceneScope = serde_json::from_str(&json).expect("deserialize");
    assert!(matches!(restored, SceneScope::Full));
}

#[test]
fn scope_pc_only_json_round_trip() {
    let scope = SceneScope::PcOnly;
    let json = serde_json::to_string(&scope).expect("serialize");
    let restored: SceneScope = serde_json::from_str(&json).expect("deserialize");
    assert!(matches!(restored, SceneScope::PcOnly));
}

#[test]
fn scope_room_only_json_round_trip() {
    let scope = SceneScope::RoomOnly;
    let json = serde_json::to_string(&scope).expect("serialize");
    let restored: SceneScope = serde_json::from_str(&json).expect("deserialize");
    assert!(matches!(restored, SceneScope::RoomOnly));
}

#[test]
fn scope_devices_holds_ids() {
    let scope = SceneScope::Devices(vec!["dev-a".into(), "dev-b".into()]);
    if let SceneScope::Devices(ids) = &scope {
        assert_eq!(ids.len(), 2);
        assert_eq!(ids[0], "dev-a");
    } else {
        panic!("Expected Devices variant");
    }
}

#[test]
fn scope_zones_holds_names() {
    let scope = SceneScope::Zones(vec!["kb:main".into(), "strip:left".into()]);
    if let SceneScope::Zones(names) = &scope {
        assert_eq!(names.len(), 2);
    } else {
        panic!("Expected Zones variant");
    }
}

#[test]
fn scope_devices_json_round_trip() {
    let scope = SceneScope::Devices(vec!["a".into(), "b".into()]);
    let json = serde_json::to_string(&scope).expect("serialize");
    let restored: SceneScope = serde_json::from_str(&json).expect("deserialize");
    if let SceneScope::Devices(ids) = restored {
        assert_eq!(ids, vec!["a", "b"]);
    } else {
        panic!("Expected Devices variant after round-trip");
    }
}

// ── ZoneAssignment ───────────────────────────────────────────────────────

#[test]
fn zone_assignment_with_brightness() {
    let za = ZoneAssignment {
        zone_name: "strip:ceiling".into(),
        effect_name: "static".into(),
        parameters: HashMap::from([("color".into(), "#e135ff".into())]),
        brightness: Some(0.5),
    };
    assert_eq!(za.brightness, Some(0.5));
    assert_eq!(
        za.parameters.get("color").map(String::as_str),
        Some("#e135ff")
    );
}

#[test]
fn zone_assignment_without_brightness() {
    let za = ZoneAssignment {
        zone_name: "mouse:scroll".into(),
        effect_name: "breathing".into(),
        parameters: HashMap::new(),
        brightness: None,
    };
    assert!(za.brightness.is_none());
    assert!(za.parameters.is_empty());
}

#[test]
fn zone_assignment_json_round_trip() {
    let original = ZoneAssignment {
        zone_name: "kb:function_row".into(),
        effect_name: "ripple".into(),
        parameters: HashMap::from([
            ("speed".into(), "0.8".into()),
            ("color".into(), "#80ffea".into()),
        ]),
        brightness: Some(1.0),
    };
    let json = serde_json::to_string(&original).expect("serialize");
    let restored: ZoneAssignment = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(restored.zone_name, original.zone_name);
    assert_eq!(restored.effect_name, original.effect_name);
    assert_eq!(restored.parameters, original.parameters);
    assert_eq!(restored.brightness, original.brightness);
}

// ── TransitionSpec ───────────────────────────────────────────────────────

#[test]
fn transition_spec_construction() {
    let spec = TransitionSpec {
        duration_ms: 2000,
        easing: EasingFunction::Linear,
        color_interpolation: ColorInterpolation::Srgb,
    };
    assert_eq!(spec.duration_ms, 2000);
    assert!(matches!(spec.easing, EasingFunction::Linear));
    assert!(matches!(spec.color_interpolation, ColorInterpolation::Srgb));
}

#[test]
fn transition_spec_json_round_trip() {
    let original = TransitionSpec {
        duration_ms: 500,
        easing: EasingFunction::CubicBezier {
            x1: 0.25,
            y1: 0.1,
            x2: 0.25,
            y2: 1.0,
        },
        color_interpolation: ColorInterpolation::Oklab,
    };
    let json = serde_json::to_string(&original).expect("serialize");
    let restored: TransitionSpec = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(restored.duration_ms, 500);
    if let EasingFunction::CubicBezier { x1, y1, x2, y2 } = restored.easing {
        assert!((x1 - 0.25).abs() < f32::EPSILON);
        assert!((y1 - 0.1).abs() < f32::EPSILON);
        assert!((x2 - 0.25).abs() < f32::EPSILON);
        assert!((y2 - 1.0).abs() < f32::EPSILON);
    } else {
        panic!("Expected CubicBezier variant");
    }
}

// ── EasingFunction ───────────────────────────────────────────────────────

#[test]
fn linear_easing_is_identity() {
    let ease = EasingFunction::Linear;
    assert!((ease.apply(0.0)).abs() < f32::EPSILON);
    assert!((ease.apply(0.5) - 0.5).abs() < f32::EPSILON);
    assert!((ease.apply(1.0) - 1.0).abs() < f32::EPSILON);
}

#[test]
fn ease_in_starts_slow() {
    let ease = EasingFunction::EaseIn;
    // At t=0.5, cubic ease-in = 0.125 (much less than 0.5)
    assert!((ease.apply(0.5) - 0.125).abs() < 1e-5);
    assert!((ease.apply(0.0)).abs() < f32::EPSILON);
    assert!((ease.apply(1.0) - 1.0).abs() < f32::EPSILON);
}

#[test]
fn ease_out_starts_fast() {
    let ease = EasingFunction::EaseOut;
    // At t=0.5, cubic ease-out = 0.875 (much more than 0.5)
    assert!((ease.apply(0.5) - 0.875).abs() < 1e-5);
    assert!((ease.apply(0.0)).abs() < f32::EPSILON);
    assert!((ease.apply(1.0) - 1.0).abs() < f32::EPSILON);
}

#[test]
fn ease_in_out_is_symmetric() {
    let ease = EasingFunction::EaseInOut;
    assert!((ease.apply(0.0)).abs() < f32::EPSILON);
    assert!((ease.apply(0.5) - 0.5).abs() < 1e-5);
    assert!((ease.apply(1.0) - 1.0).abs() < f32::EPSILON);

    // Symmetry: f(t) + f(1-t) = 1
    for i in 1_u8..10 {
        let t = f32::from(i) / 10.0;
        let sum = ease.apply(t) + ease.apply(1.0 - t);
        assert!((sum - 1.0).abs() < 1e-5, "Symmetry broken at t={t}");
    }
}

#[test]
fn ease_in_out_slow_at_extremes() {
    let ease = EasingFunction::EaseInOut;
    // First half should be below 0.5
    assert!(ease.apply(0.25) < 0.5);
    // Second half should be above 0.5
    assert!(ease.apply(0.75) > 0.5);
}

#[test]
fn easing_clamps_input_below_zero() {
    let ease = EasingFunction::Linear;
    assert!((ease.apply(-0.5)).abs() < f32::EPSILON);
}

#[test]
fn easing_clamps_input_above_one() {
    let ease = EasingFunction::Linear;
    assert!((ease.apply(1.5) - 1.0).abs() < f32::EPSILON);
}

#[test]
fn cubic_bezier_ease_endpoints() {
    // CSS ease: cubic-bezier(0.25, 0.1, 0.25, 1.0)
    let ease = EasingFunction::CubicBezier {
        x1: 0.25,
        y1: 0.1,
        x2: 0.25,
        y2: 1.0,
    };
    assert!((ease.apply(0.0)).abs() < 1e-4);
    assert!((ease.apply(1.0) - 1.0).abs() < 1e-4);
}

#[test]
fn cubic_bezier_linear_when_diagonal() {
    // cubic-bezier(0.5, 0.5, 0.5, 0.5) should approximate linear
    let ease = EasingFunction::CubicBezier {
        x1: 0.5,
        y1: 0.5,
        x2: 0.5,
        y2: 0.5,
    };
    // Midpoint should be close to 0.5
    assert!((ease.apply(0.5) - 0.5).abs() < 0.05);
}

#[test]
fn cubic_bezier_monotonic_for_standard_curves() {
    let ease = EasingFunction::CubicBezier {
        x1: 0.42,
        y1: 0.0,
        x2: 0.58,
        y2: 1.0,
    };
    let mut prev = 0.0_f32;
    for i in 0_u8..=20 {
        let t = f32::from(i) / 20.0;
        let val = ease.apply(t);
        assert!(val >= prev - 1e-4, "Not monotonic at t={t}: {val} < {prev}");
        prev = val;
    }
}

#[test]
fn all_easing_variants_json_round_trip() {
    let variants = vec![
        EasingFunction::Linear,
        EasingFunction::EaseIn,
        EasingFunction::EaseOut,
        EasingFunction::EaseInOut,
        EasingFunction::CubicBezier {
            x1: 0.42,
            y1: 0.0,
            x2: 0.58,
            y2: 1.0,
        },
    ];
    for variant in &variants {
        let json = serde_json::to_string(variant).expect("serialize EasingFunction");
        let restored: EasingFunction =
            serde_json::from_str(&json).expect("deserialize EasingFunction");
        // Verify endpoints match
        assert!((variant.apply(0.0) - restored.apply(0.0)).abs() < 1e-5);
        assert!((variant.apply(1.0) - restored.apply(1.0)).abs() < 1e-5);
        assert!((variant.apply(0.5) - restored.apply(0.5)).abs() < 1e-5);
    }
}

// ── ColorInterpolation ───────────────────────────────────────────────────

#[test]
fn color_interpolation_json_round_trip() {
    for variant in [ColorInterpolation::Srgb, ColorInterpolation::Oklab] {
        let json = serde_json::to_string(&variant).expect("serialize");
        let restored: ColorInterpolation = serde_json::from_str(&json).expect("deserialize");
        // Check discriminant matches via debug string
        assert_eq!(format!("{variant:?}"), format!("{restored:?}"));
    }
}

// ── ScenePriority ────────────────────────────────────────────────────────

#[test]
fn priority_constants_have_correct_values() {
    assert_eq!(ScenePriority::AMBIENT.0, 0);
    assert_eq!(ScenePriority::USER.0, 50);
    assert_eq!(ScenePriority::TRIGGER.0, 75);
    assert_eq!(ScenePriority::ALERT.0, 100);
}

#[test]
fn priority_ordering() {
    assert!(ScenePriority::AMBIENT < ScenePriority::USER);
    assert!(ScenePriority::USER < ScenePriority::TRIGGER);
    assert!(ScenePriority::TRIGGER < ScenePriority::ALERT);
}

#[test]
fn priority_default_is_user() {
    assert_eq!(ScenePriority::default(), ScenePriority::USER);
}

#[test]
fn priority_custom_value() {
    let custom = ScenePriority(42);
    assert!(custom > ScenePriority::AMBIENT);
    assert!(custom < ScenePriority::USER);
}

#[test]
fn priority_display_named_tiers() {
    assert_eq!(ScenePriority::AMBIENT.to_string(), "ambient");
    assert_eq!(ScenePriority::USER.to_string(), "user");
    assert_eq!(ScenePriority::TRIGGER.to_string(), "trigger");
    assert_eq!(ScenePriority::ALERT.to_string(), "alert");
}

#[test]
fn priority_display_custom_value() {
    let custom = ScenePriority(42);
    assert_eq!(custom.to_string(), "priority(42)");
}

#[test]
fn priority_json_round_trip() {
    let original = ScenePriority::TRIGGER;
    let json = serde_json::to_string(&original).expect("serialize");
    let restored: ScenePriority = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(original, restored);
}

// ── TriggerSource ────────────────────────────────────────────────────────

#[test]
fn trigger_time_of_day() {
    let trigger = TriggerSource::TimeOfDay {
        hour: 22,
        minute: 30,
    };
    if let TriggerSource::TimeOfDay { hour, minute } = trigger {
        assert_eq!(hour, 22);
        assert_eq!(minute, 30);
    } else {
        panic!("Expected TimeOfDay variant");
    }
}

#[test]
fn trigger_app_launched() {
    let trigger = TriggerSource::AppLaunched("firefox".into());
    if let TriggerSource::AppLaunched(app) = &trigger {
        assert_eq!(app, "firefox");
    } else {
        panic!("Expected AppLaunched variant");
    }
}

#[test]
fn trigger_audio_level() {
    let trigger = TriggerSource::AudioLevel { threshold: 0.75 };
    if let TriggerSource::AudioLevel { threshold } = trigger {
        assert!((threshold - 0.75).abs() < f32::EPSILON);
    } else {
        panic!("Expected AudioLevel variant");
    }
}

#[test]
fn trigger_source_json_round_trip() {
    let variants = vec![
        TriggerSource::TimeOfDay { hour: 9, minute: 0 },
        TriggerSource::Sunset,
        TriggerSource::Sunrise,
        TriggerSource::AppLaunched("steam".into()),
        TriggerSource::AudioLevel { threshold: 0.5 },
        TriggerSource::GameDetected,
        TriggerSource::Manual,
    ];
    for variant in &variants {
        let json = serde_json::to_string(variant).expect("serialize TriggerSource");
        let _restored: TriggerSource =
            serde_json::from_str(&json).expect("deserialize TriggerSource");
    }
}

// ── AutomationRule ───────────────────────────────────────────────────────

#[test]
fn automation_rule_construction() {
    let rule = AutomationRule {
        name: "Gaming Mode".into(),
        trigger: TriggerSource::GameDetected,
        conditions: vec!["after_sunset".into()],
        action: ActionKind::ActivateScene("gaming-reactive".into()),
        cooldown_secs: 30,
        enabled: true,
    };
    assert_eq!(rule.name, "Gaming Mode");
    assert_eq!(rule.cooldown_secs, 30);
    assert!(rule.enabled);
    assert_eq!(rule.conditions.len(), 1);
}

#[test]
fn automation_rule_with_no_conditions() {
    let rule = AutomationRule {
        name: "Always On".into(),
        trigger: TriggerSource::Manual,
        conditions: vec![],
        action: ActionKind::RestorePrevious,
        cooldown_secs: 0,
        enabled: true,
    };
    assert!(rule.conditions.is_empty());
    assert_eq!(rule.cooldown_secs, 0);
}

#[test]
fn automation_rule_json_round_trip() {
    let original = AutomationRule {
        name: "Night Mode".into(),
        trigger: TriggerSource::TimeOfDay {
            hour: 22,
            minute: 0,
        },
        conditions: vec!["screen_unlocked".into(), "not_gaming".into()],
        action: ActionKind::ActivateScene("cozy-evening".into()),
        cooldown_secs: 300,
        enabled: true,
    };
    let json = serde_json::to_string(&original).expect("serialize");
    let restored: AutomationRule = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(restored.name, original.name);
    assert_eq!(restored.cooldown_secs, original.cooldown_secs);
    assert_eq!(restored.conditions, original.conditions);
    assert_eq!(restored.enabled, original.enabled);
}

// ── ActionKind ───────────────────────────────────────────────────────────

#[test]
fn action_activate_scene() {
    let action = ActionKind::ActivateScene("celebration".into());
    if let ActionKind::ActivateScene(name) = &action {
        assert_eq!(name, "celebration");
    } else {
        panic!("Expected ActivateScene variant");
    }
}

#[test]
fn action_set_brightness() {
    let action = ActionKind::SetBrightness(0.3);
    if let ActionKind::SetBrightness(b) = action {
        assert!((b - 0.3).abs() < f32::EPSILON);
    } else {
        panic!("Expected SetBrightness variant");
    }
}

#[test]
fn action_restore_previous() {
    let action = ActionKind::RestorePrevious;
    assert!(matches!(action, ActionKind::RestorePrevious));
}

#[test]
fn action_kind_json_round_trip() {
    let variants = vec![
        ActionKind::ActivateScene("test".into()),
        ActionKind::SetBrightness(0.75),
        ActionKind::RestorePrevious,
    ];
    for variant in &variants {
        let json = serde_json::to_string(variant).expect("serialize ActionKind");
        let _restored: ActionKind = serde_json::from_str(&json).expect("deserialize ActionKind");
    }
}

// ── TOML Serialization ──────────────────────────────────────────────────

#[test]
fn scene_toml_round_trip() {
    let original = sample_scene();
    let toml_str = toml::to_string(&original).expect("serialize Scene to TOML");
    let restored: Scene = toml::from_str(&toml_str).expect("deserialize Scene from TOML");
    assert_eq!(restored.name, original.name);
    assert_eq!(restored.enabled, original.enabled);
}

#[test]
fn automation_rule_toml_round_trip() {
    let original = AutomationRule {
        name: "Sunset Warmth".into(),
        trigger: TriggerSource::Sunset,
        conditions: vec![],
        action: ActionKind::ActivateScene("warm-ambient".into()),
        cooldown_secs: 600,
        enabled: true,
    };
    let toml_str = toml::to_string(&original).expect("serialize to TOML");
    let restored: AutomationRule = toml::from_str(&toml_str).expect("deserialize from TOML");
    assert_eq!(restored.name, original.name);
    assert_eq!(restored.cooldown_secs, original.cooldown_secs);
}
