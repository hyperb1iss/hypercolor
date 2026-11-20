use hypercolor_types::config::{HypercolorConfig, InteractionRoutePolicy};
use hypercolor_ui::api::ApiError;
use hypercolor_ui::config_state::{
    ConfigApplyTracker, ConfigWrite, DEV_CAPTURE_UNLOCK_HINT, apply_config_key,
    capture_lock_notice, config_key_value, config_setting_label, config_write_failure_message,
    is_capture_credential_refusal, schema_entry_for, schema_requires_protected_control,
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

// ── Protected keys ──────────────────────────────────────────────────

/// The lock a settings row shows comes from the wire schema, and the
/// daemon decides the refusal from its registry. Both read one table
/// here, so the two rules cannot drift apart unnoticed.
#[test]
fn schema_protection_agrees_with_the_registry_it_projects() {
    let entries = hypercolor_types::config_registry::schema_entries();

    for key in [
        "audio",
        "audio.enabled",
        "audio.device",
        "audio.fft_size",
        "audio.smoothing",
        "capture",
        "capture.enabled",
        "capture.source",
        "capture.saturation",
        "capture.calibration",
        "input",
        "input.enabled",
        "input.keyboard",
        "input.mouse",
        "input.daemon_route",
        "daemon.port",
        "daemon.target_fps",
        "network.access_mode",
        "drivers.wled.known_ips",
        "some_extension.setting",
    ] {
        assert_eq!(
            schema_requires_protected_control(&entries, key),
            hypercolor_types::config_registry::requires_protected_control(key),
            "schema protection disagrees with the registry for {key}"
        );
    }
}

#[test]
fn audio_device_is_protected_while_audio_tuning_stays_open() {
    let entries = hypercolor_types::config_registry::schema_entries();

    assert!(schema_requires_protected_control(&entries, "audio.device"));
    assert!(schema_requires_protected_control(&entries, "audio.enabled"));
    assert!(schema_requires_protected_control(&entries, "audio"));
    assert!(!schema_requires_protected_control(
        &entries,
        "audio.fft_size"
    ));
    assert!(!schema_requires_protected_control(&entries, "daemon.port"));
}

#[test]
fn nothing_is_protected_before_the_schema_arrives() {
    assert!(!schema_requires_protected_control(&[], "audio.device"));
}

// ── Refused writes ──────────────────────────────────────────────────

fn refusal(status: u16, code: &str, message: &str) -> ApiError {
    ApiError::Http {
        status,
        code: Some(code.to_owned()),
        message: Some(message.to_owned()),
        details: None,
    }
}

#[test]
fn capture_setting_refused_for_want_of_a_credential_says_where_to_change_it() {
    let error = refusal(
        403,
        "forbidden",
        "Protected capture access requires a control credential",
    );

    assert!(is_capture_credential_refusal("audio.device", &error));
    assert_eq!(
        config_write_failure_message("audio.device", ConfigWrite::Change, &error, false),
        "Audio device wasn't changed. Capture settings can only be changed from the Hypercolor app."
    );
}

#[test]
fn the_refusal_never_repeats_the_daemon_s_credential_wording() {
    let error = refusal(
        403,
        "forbidden",
        "Protected capture access requires a control credential",
    );

    for key in [
        "audio.enabled",
        "capture.saturation",
        "input.mouse",
        "capture",
    ] {
        let message = config_write_failure_message(key, ConfigWrite::Change, &error, false);
        assert!(!message.contains("credential"), "{message}");
        assert!(!message.contains("Protected"), "{message}");
        assert!(!message.contains("HTTP"), "{message}");
    }
}

#[test]
fn a_dev_build_adds_how_a_dev_stack_unlocks_capture_settings() {
    let error = refusal(
        403,
        "forbidden",
        "Protected capture access requires a control credential",
    );

    let message = config_write_failure_message("audio.device", ConfigWrite::Change, &error, true);
    assert!(message.starts_with("Audio device wasn't changed. Capture settings can only"));
    assert!(message.ends_with("open the protected controls link that just dev prints."));
}

#[test]
fn other_refusals_carry_the_daemon_s_message() {
    let invalid = refusal(422, "validation_error", "fft_size must be a power of two");
    assert!(!is_capture_credential_refusal("audio.fft_size", &invalid));
    assert_eq!(
        config_write_failure_message("audio.fft_size", ConfigWrite::Change, &invalid, false),
        "Audio fft size wasn't changed: fft_size must be a power of two"
    );

    // A 403 on a key that needs no capture credential is some other
    // refusal, such as a read-only API key, and keeps the daemon's words.
    let read_only = refusal(403, "forbidden", "This API key is read-only");
    assert!(!is_capture_credential_refusal("daemon.port", &read_only));
    assert_eq!(
        config_write_failure_message("daemon.port", ConfigWrite::Change, &read_only, true),
        "Daemon port wasn't changed: This API key is read-only"
    );

    // A capture key turned down for a different reason is not a
    // credential problem either.
    let conflict = refusal(409, "conflict", "Capture is busy");
    assert_eq!(
        config_write_failure_message("capture.source", ConfigWrite::Change, &conflict, false),
        "Capture source wasn't changed: Capture is busy"
    );
}

#[test]
fn a_write_that_never_reached_the_daemon_says_so() {
    let offline = ApiError::Network("Failed to fetch".to_owned());
    assert_eq!(
        config_write_failure_message("audio.device", ConfigWrite::Change, &offline, false),
        "Audio device wasn't changed: Network error: Failed to fetch"
    );
}

#[test]
fn setting_labels_read_as_names_not_keys() {
    assert_eq!(config_setting_label("audio.enabled"), "Audio capture");
    assert_eq!(config_setting_label("input.keyboard"), "Keyboard input");
    assert_eq!(
        config_setting_label("capture"),
        "The Screen Capture section"
    );
    assert_eq!(config_setting_label("capture.capture_fps"), "Capture fps");
    assert_eq!(
        config_setting_label("daemon.target_fps"),
        "Daemon target fps"
    );
    assert_eq!(
        config_setting_label("drivers.wled.known_ips"),
        "Drivers known ips"
    );
    assert_eq!(config_setting_label("mcp"), "Mcp");
}

#[test]
fn a_refused_section_reset_says_reset_and_stays_singular() {
    let error = refusal(
        403,
        "forbidden",
        "Protected capture access requires a control credential",
    );

    assert_eq!(
        config_write_failure_message("audio", ConfigWrite::Reset, &error, false),
        "The Audio section wasn't reset. Capture settings can only be changed from the Hypercolor app."
    );
    assert_eq!(
        config_write_failure_message("capture.calibration", ConfigWrite::Reset, &error, false),
        "Capture calibration wasn't reset. Capture settings can only be changed from the Hypercolor app."
    );

    let invalid = refusal(409, "conflict", "A snapshot scene is active");
    assert_eq!(
        config_write_failure_message("network", ConfigWrite::Reset, &invalid, false),
        "Network wasn't reset: A snapshot scene is active"
    );
}

// ── Lock notice ─────────────────────────────────────────────────────

#[test]
fn a_release_build_s_lock_notice_names_only_the_app() {
    assert_eq!(
        capture_lock_notice("Screen capture settings", false),
        "Screen capture settings are locked in this browser. Change them from the Hypercolor app."
    );
    assert!(
        !capture_lock_notice("The audio capture switch and device", false).contains("Dev stack")
    );
}

#[test]
fn a_dev_build_s_lock_notice_points_at_the_link_just_dev_prints() {
    assert_eq!(
        capture_lock_notice("The audio capture switch and device", true),
        "The audio capture switch and device are locked in this browser. Change them from the Hypercolor app. Dev stack: open the protected controls link that just dev prints."
    );
}

/// A locked row never reaches the refusal toast, so the notice has to
/// carry the same pointer the toast does, word for word.
#[test]
fn the_lock_notice_and_the_refusal_toast_share_one_dev_hint() {
    let error = refusal(
        403,
        "forbidden",
        "Protected capture access requires a control credential",
    );
    let toast = config_write_failure_message("audio.device", ConfigWrite::Change, &error, true);
    let notice = capture_lock_notice("The audio capture switch and device", true);

    assert!(toast.ends_with(DEV_CAPTURE_UNLOCK_HINT));
    assert!(notice.ends_with(DEV_CAPTURE_UNLOCK_HINT));
    assert_eq!(
        DEV_CAPTURE_UNLOCK_HINT,
        "Dev stack: open the protected controls link that just dev prints."
    );
}

// ── Which 403 it is ─────────────────────────────────────────────────

fn refusal_with_details(message: &str, details: serde_json::Value) -> ApiError {
    ApiError::Http {
        status: 403,
        code: Some("forbidden".to_owned()),
        message: Some(message.to_owned()),
        details: Some(details),
    }
}

/// The capture gate answers with a bare 403: the shape a keyless
/// browser, or a read-only key on the same machine, gets back.
#[test]
fn a_bare_403_on_a_capture_key_is_the_capture_credential_refusal() {
    let error = refusal(
        403,
        "forbidden",
        "Protected capture access requires a control credential",
    );
    assert!(is_capture_credential_refusal("audio.device", &error));

    let null_details = refusal_with_details(
        "Protected capture access requires a control credential",
        serde_json::Value::Null,
    );
    assert!(is_capture_credential_refusal("audio.device", &null_details));
}

/// A read-only API key sent from another machine never reaches the
/// capture gate: the tier check answers first, and says what is wrong.
#[test]
fn a_read_only_key_refusal_on_a_capture_key_keeps_the_daemon_s_message() {
    let error = refusal_with_details(
        "Read-only API key cannot perform write operations",
        serde_json::json!({ "required_tier": "control", "current_tier": "read" }),
    );

    assert!(!is_capture_credential_refusal("audio.device", &error));
    for dev_build in [false, true] {
        assert_eq!(
            config_write_failure_message("audio.device", ConfigWrite::Change, &error, dev_build),
            "Audio device wasn't changed: Read-only API key cannot perform write operations"
        );
    }
}

/// Any other gate that explains itself in details is not the capture
/// gate either.
#[test]
fn a_403_with_other_details_on_a_capture_key_keeps_the_daemon_s_message() {
    let error = refusal_with_details(
        "Requests relayed by a reverse proxy need an API key",
        serde_json::json!({ "setting": "network.access_mode" }),
    );

    assert!(!is_capture_credential_refusal("capture.enabled", &error));
    assert_eq!(
        config_write_failure_message("capture.enabled", ConfigWrite::Change, &error, false),
        "Screen capture wasn't changed: Requests relayed by a reverse proxy need an API key"
    );
}
