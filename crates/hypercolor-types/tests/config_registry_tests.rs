//! Completeness and lookup coverage for the config-key registry
//! (Spec 76 §3.3). The completeness test is the contract: a config
//! section added without a registry row fails here, not in review.

use hypercolor_types::config::HypercolorConfig;
use hypercolor_types::config_registry::{
    ApplyPolicy, LiveSection, Redaction, declared_namespace_roots, declared_section_roots,
    descriptor_for, is_redacted, registry, requires_restart, schema_entries,
};

fn config_top_level_keys() -> Vec<String> {
    let json = serde_json::to_value(HypercolorConfig::default()).expect("config serializes");
    json.as_object()
        .expect("config is an object")
        .keys()
        .cloned()
        .collect()
}

#[test]
fn every_config_section_has_a_registry_owner() {
    for key in config_top_level_keys() {
        let descriptor = descriptor_for(&key);
        assert!(
            !matches!(
                descriptor.pattern,
                hypercolor_types::config_registry::KeyPattern::ExtensionsCatchAll
            ),
            "top-level section `{key}` fell through to the extensions catch-all — \
             add a registry row for it"
        );
    }
}

#[test]
fn every_registry_root_is_a_real_config_key() {
    let keys = config_top_level_keys();
    for root in declared_section_roots() {
        assert!(
            keys.iter().any(|key| key == root),
            "registry declares section `{root}` which HypercolorConfig does not serialize — \
             stale registry row"
        );
    }
    for root in declared_namespace_roots() {
        assert!(
            keys.iter().any(|key| key == root),
            "registry declares namespace `{root}` which HypercolorConfig does not serialize"
        );
    }
}

#[test]
fn lookup_picks_the_most_specific_pattern() {
    assert_eq!(
        descriptor_for("daemon.target_fps").apply,
        ApplyPolicy::Live(LiveSection::Render)
    );
    assert_eq!(descriptor_for("daemon.port").apply, ApplyPolicy::Restart);
    assert_eq!(descriptor_for("daemon").apply, ApplyPolicy::Restart);
    assert_eq!(
        descriptor_for("audio.device").apply,
        ApplyPolicy::Live(LiveSection::Audio)
    );
    assert_eq!(
        descriptor_for("discovery.mdns_enabled").apply,
        ApplyPolicy::Restart
    );
    assert_eq!(
        descriptor_for("discovery.blocks_scan").apply,
        ApplyPolicy::NextScan
    );
    assert_eq!(
        descriptor_for("drivers.wled.known_ips").apply,
        ApplyPolicy::LiveOnRead
    );
    // A section this build does not model lands on the catch-all.
    assert_eq!(descriptor_for("cloud.token").apply, ApplyPolicy::Inert);
}

#[test]
fn dynamic_namespaces_redact_deny_by_default() {
    assert_eq!(
        descriptor_for("drivers.govee.api_key").redaction,
        Redaction::Secret
    );
    assert_eq!(descriptor_for("cloud.token").redaction, Redaction::Secret);
    assert_eq!(descriptor_for("audio.device").redaction, Redaction::Plain);
    assert!(is_redacted("drivers.anything.at_all"));
    assert!(!is_redacted("daemon.target_fps"));
}

#[test]
fn restart_classification_matches_reality() {
    assert!(requires_restart("network.access_mode"));
    assert!(requires_restart("mcp.enabled"));
    assert!(requires_restart("daemon.listen_address"));
    assert!(!requires_restart("daemon.target_fps"));
    assert!(!requires_restart("audio.device"));
    // The session watcher only spawns at boot; enabling after a
    // disabled start needs a restart.
    assert!(requires_restart("session.enabled"));
    assert!(!requires_restart("session.sleep_behavior"));
    // Read per effect-error event, unlike its frozen section.
    assert_eq!(
        descriptor_for("effect_engine.effect_error_fallback").apply,
        ApplyPolicy::LiveOnRead
    );
    assert_eq!(
        descriptor_for("effect_engine.compositor_acceleration_mode").apply,
        ApplyPolicy::Restart
    );
}

#[test]
fn key_grammar_rejects_empty_segments() {
    use hypercolor_types::config_registry::is_valid_key;
    assert!(is_valid_key("daemon.port"));
    assert!(is_valid_key("drivers.wled.known_ips"));
    for bad in ["", ".", "audio.", ".audio", "a..b"] {
        assert!(!is_valid_key(bad), "{bad:?} must be rejected");
    }
}

#[test]
fn capture_validator_is_wired_and_rejects_bad_values() {
    let validate = descriptor_for("capture")
        .validate
        .expect("capture section declares a validator");
    let valid =
        serde_json::to_value(HypercolorConfig::default().capture).expect("capture serializes");
    assert!(validate(&valid).is_ok());
    let mut invalid = valid;
    invalid["smoothing"] = serde_json::json!(5.0);
    assert!(
        validate(&invalid).is_err(),
        "out-of-range smoothing must be rejected"
    );
}

#[test]
fn schema_entries_project_every_row() {
    let entries = schema_entries();
    assert_eq!(entries.len(), registry().len());
    assert!(entries.iter().any(|entry| entry.pattern == "*"));
    assert!(entries.iter().any(|entry| entry.pattern == "drivers.*"));
    let capture = entries
        .iter()
        .find(|entry| entry.pattern == "capture")
        .expect("capture row present");
    assert!(capture.has_validator);
}
