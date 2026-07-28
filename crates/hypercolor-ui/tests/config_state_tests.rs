use hypercolor_types::config::{HypercolorConfig, InteractionRoutePolicy};
use hypercolor_ui::config_state::{ConfigApplyTracker, apply_config_key, config_key_value};

#[test]
fn config_values_roundtrip_for_rollback() {
    let mut config = HypercolorConfig::default();
    let previous = config_key_value(&config, "input.keyboard").expect("input keyboard value");

    apply_config_key(&mut config, "input.keyboard", &serde_json::json!(false));
    assert_eq!(
        config_key_value(&config, "input.keyboard"),
        Some(false.into())
    );

    apply_config_key(&mut config, "input.keyboard", &previous);
    assert_eq!(config_key_value(&config, "input.keyboard"), Some(previous));
}

#[test]
fn rollback_generation_never_overwrites_a_newer_same_key_apply() {
    let mut tracker = ConfigApplyTracker::default();
    let mut config = HypercolorConfig::default();
    let stale = tracker.begin("input.keyboard");
    apply_config_key(&mut config, "input.keyboard", &serde_json::json!(false));
    let current = tracker.begin("input.keyboard");
    apply_config_key(&mut config, "input.keyboard", &serde_json::json!(true));

    assert!(!tracker.finish_if_current("input.keyboard", stale));
    assert!(config.input.keyboard);
    assert!(tracker.finish_if_current("input.keyboard", current));
    assert!(!tracker.finish_if_current("input.keyboard", current));
}

#[test]
fn failed_independent_key_rollback_preserves_newer_optimistic_state() {
    let mut tracker = ConfigApplyTracker::default();
    let mut config = HypercolorConfig::default();
    let previous_keyboard = config_key_value(&config, "input.keyboard").expect("keyboard value");
    let keyboard = tracker.begin("input.keyboard");
    apply_config_key(&mut config, "input.keyboard", &serde_json::json!(false));
    let mouse = tracker.begin("input.mouse");
    apply_config_key(&mut config, "input.mouse", &serde_json::json!(false));

    assert!(tracker.finish_if_current("input.keyboard", keyboard));
    apply_config_key(&mut config, "input.keyboard", &previous_keyboard);
    assert!(config.input.keyboard);
    assert!(!config.input.mouse);
    assert!(tracker.finish_if_current("input.mouse", mouse));
}

#[test]
fn input_controls_dispatch_independent_toggles_and_routes() {
    let mut config = HypercolorConfig::default();

    apply_config_key(&mut config, "input.keyboard", &serde_json::json!(false));
    apply_config_key(&mut config, "input.mouse", &serde_json::json!(true));
    apply_config_key(
        &mut config,
        "input.daemon_route",
        &serde_json::json!("browser"),
    );
    apply_config_key(
        &mut config,
        "input.preview_route",
        &serde_json::json!("merge"),
    );

    assert!(!config.input.keyboard);
    assert!(config.input.mouse);
    assert_eq!(config.input.daemon_route, InteractionRoutePolicy::Browser);
    assert_eq!(config.input.preview_route, InteractionRoutePolicy::Merge);
}
