use hypercolor_types::config::{EffectErrorFallbackPolicy, HypercolorConfig};
use hypercolor_types::portable_setup::PortablePreferences;

#[test]
fn portable_preferences_copy_taste_and_preserve_machine_configuration() {
    let mut source = HypercolorConfig::default();
    source.audio.device = "source private endpoint".into();
    source.audio.smoothing = 0.42;
    source.capture.restore_token = Some("private token".into());
    source.capture.saturation = 1.7;
    source.effect_engine.effect_error_fallback = EffectErrorFallbackPolicy::ClearZones;
    let preferences = PortablePreferences::from_config(&source);
    let serialized =
        serde_json::to_string(&preferences).expect("portable backup fixture should be valid");
    assert!(!serialized.contains("private"));
    assert!(!serialized.contains("enabled"));
    assert!(serialized.contains("clear_zones"));
    let mut target = HypercolorConfig::default();
    target.audio.device = "destination device".into();
    target.capture.restore_token = Some("destination token".into());
    let original = serde_json::to_value(&target).expect("portable backup fixture should be valid");
    preferences
        .apply_to_config(&mut target)
        .expect("portable backup fixture should be valid");
    let after = serde_json::to_value(&target).expect("portable backup fixture should be valid");
    assert_eq!(after["audio"]["device"], original["audio"]["device"]);
    assert_eq!(
        after["capture"]["restore_token"],
        original["capture"]["restore_token"]
    );
    assert_eq!(after["drivers"], original["drivers"]);
    assert_eq!(after["network"], original["network"]);
    assert_eq!(after["extensions"], original["extensions"]);
    assert_eq!(
        after["audio"]["smoothing"],
        serde_json::json!(source.audio.smoothing)
    );
    assert_eq!(
        after["capture"]["saturation"],
        serde_json::json!(source.capture.saturation)
    );
}

#[test]
fn preference_validation_rejects_unknown_private_fields_and_invalid_values_before_apply() {
    let mut preferences = PortablePreferences::from_config(&HypercolorConfig::default());
    let mut value =
        serde_json::to_value(&preferences).expect("portable backup fixture should be valid");
    value["audio"]["device"] = "private endpoint".into();
    assert!(serde_json::from_value::<PortablePreferences>(value).is_err());
    preferences
        .capture
        .as_mut()
        .expect("portable backup fixture should be valid")
        .gamma = f32::NAN;
    let mut target = HypercolorConfig::default();
    let original = serde_json::to_value(&target).expect("portable backup fixture should be valid");
    assert!(preferences.apply_to_config(&mut target).is_err());
    assert_eq!(
        serde_json::to_value(&target).expect("portable backup fixture should be valid"),
        original
    );
    preferences
        .capture
        .as_mut()
        .expect("portable backup fixture should be valid")
        .gamma = 1.0;
    preferences
        .shutdown
        .as_mut()
        .expect("portable backup fixture should be valid")
        .color = "#oops".into();
    assert!(preferences.validate().is_err());
}
