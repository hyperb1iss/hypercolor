use hypercolor_types::device::{DeviceCapabilities, DeviceFeatures};

#[test]
fn device_features_default_to_all_disabled() {
    let features = DeviceFeatures::default();

    assert!(!features.scroll_mode);
    assert!(!features.scroll_smart_reel);
    assert!(!features.scroll_acceleration);
}

#[test]
fn device_features_serde_round_trip() {
    let features = DeviceFeatures {
        scroll_mode: true,
        scroll_smart_reel: true,
        scroll_acceleration: false,
    };

    let json = serde_json::to_string(&features).expect("serialize device features");
    let back: DeviceFeatures = serde_json::from_str(&json).expect("deserialize device features");

    assert_eq!(back, features);
}

#[test]
fn device_capabilities_back_compat_defaults_missing_features() {
    let json = r#"{
        "led_count": 11,
        "supports_direct": true,
        "supports_brightness": true,
        "has_display": false,
        "display_resolution": null,
        "max_fps": 120
    }"#;

    let capabilities: DeviceCapabilities =
        serde_json::from_str(json).expect("deserialize legacy capabilities");

    assert_eq!(capabilities.led_count, 11);
    assert_eq!(capabilities.features, DeviceFeatures::default());
}
