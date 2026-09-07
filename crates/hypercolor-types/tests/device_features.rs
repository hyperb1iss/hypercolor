use hypercolor_types::device::{DeviceCapabilities, DeviceColorSpace, DeviceFeatures};

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
        max_display_frame_len: None,
    };

    let json = serde_json::to_string(&features).expect("serialize device features");
    let back: DeviceFeatures = serde_json::from_str(&json).expect("deserialize device features");

    assert_eq!(back, features);
}

#[test]
fn device_capabilities_round_trip_and_require_current_fields() {
    let capabilities = DeviceCapabilities {
        led_count: 11,
        supports_direct: true,
        supports_brightness: true,
        has_display: false,
        display_resolution: None,
        max_fps: 120,
        color_space: DeviceColorSpace::CieXy,
        features: DeviceFeatures {
            scroll_mode: true,
            scroll_smart_reel: false,
            scroll_acceleration: true,
            max_display_frame_len: None,
        },
    };
    let json = serde_json::to_value(capabilities).expect("serialize capabilities");
    let roundtrip: DeviceCapabilities =
        serde_json::from_value(json.clone()).expect("deserialize capabilities");
    assert_eq!(roundtrip, capabilities);

    for field in ["color_space", "features"] {
        let mut missing_field = json.clone();
        missing_field
            .as_object_mut()
            .expect("capabilities are an object")
            .remove(field);
        let error = serde_json::from_value::<DeviceCapabilities>(missing_field)
            .expect_err("current capability field is required");
        let expected = format!("missing field `{field}`");
        assert!(error.to_string().contains(&expected), "{error}");
    }
}
