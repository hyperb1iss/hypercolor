use hypercolor_types::config::{HypercolorConfig, InteractionRoutePolicy};
use hypercolor_ui::config_state::{
    ConfigApplyTracker, apply_config_key, config_key_value, schema_entry_for,
    schema_requires_restart,
};

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

// ── Config schema mirror ────────────────────────────────────────────

/// The UI's pattern matcher has to agree with the daemon's registry
/// lookup on every key it renders a badge for. Both sides read the same
/// table here, so a divergence in the matching rules fails loudly.
#[test]
fn schema_lookup_agrees_with_the_registry_it_projects() {
    let entries = hypercolor_types::config_registry::schema_entries();

    for key in [
        "daemon.port",
        "daemon.target_fps",
        "daemon.canvas_width",
        "audio",
        "audio.device",
        "capture.enabled",
        "input.enabled",
        "discovery.mdns_enabled",
        "discovery.scan_interval_secs",
        "session.enabled",
        "session.sleep_behavior",
        "media.stream_private_network_allowlist",
        "effect_engine.compositor_acceleration_mode",
        "effect_engine.effect_error_fallback",
        "network.access_mode",
        "web.open_browser",
        "mcp.enabled",
        "drivers.wled.known_ips",
        "cloud.account",
    ] {
        assert_eq!(
            schema_requires_restart(&entries, key),
            hypercolor_types::config_registry::requires_restart(key),
            "schema lookup disagrees with the registry for {key}"
        );
    }
}

#[test]
fn schema_lookup_prefers_the_most_specific_pattern() {
    let entries = hypercolor_types::config_registry::schema_entries();

    let exact = schema_entry_for(&entries, "daemon.target_fps").expect("exact row exists");
    assert_eq!(exact.pattern, "daemon.target_fps");

    let section = schema_entry_for(&entries, "daemon.port").expect("section row covers the key");
    assert_eq!(section.pattern, "daemon");

    let namespace =
        schema_entry_for(&entries, "drivers.wled.known_ips").expect("namespace row covers the key");
    assert_eq!(namespace.pattern, "drivers.*");

    let catch_all =
        schema_entry_for(&entries, "some_extension.setting").expect("catch-all covers the key");
    assert_eq!(catch_all.pattern, "*");
}

#[test]
fn an_empty_schema_claims_nothing() {
    assert!(!schema_requires_restart(&[], "daemon.port"));
    assert_eq!(schema_entry_for(&[], "daemon.port"), None);
}
