use hypercolor_driver_govee::{
    GoveeCapabilities, known_cloud_sku_count, known_sku_count, profile_for_sku,
};

#[test]
fn h6163_is_basic_lan_not_streaming() {
    let profile = profile_for_sku("h6163").expect("H6163 profile should exist");

    assert!(profile.capabilities.contains(GoveeCapabilities::LAN));
    assert!(profile.capabilities.contains(GoveeCapabilities::CLOUD));
    assert!(profile.capabilities.contains(GoveeCapabilities::COLOR_RGB));
    assert!(
        profile
            .capabilities
            .contains(GoveeCapabilities::COLOR_KELVIN)
    );
    assert!(profile.capabilities.contains(GoveeCapabilities::BRIGHTNESS));
    assert!(profile.capabilities.contains(GoveeCapabilities::ON_OFF));
    assert!(!profile.capabilities.contains(GoveeCapabilities::SEGMENTS));
    assert!(
        !profile
            .capabilities
            .contains(GoveeCapabilities::RAZER_STREAMING)
    );
    assert_eq!(profile.lan_segment_count, None);
    assert_eq!(profile.razer_led_count, None);
}

#[test]
fn h619a_keeps_lan_segments_separate_from_razer_leds() {
    let profile = profile_for_sku("H619A").expect("H619A profile should exist");

    assert!(profile.capabilities.contains(GoveeCapabilities::SEGMENTS));
    assert!(
        profile
            .capabilities
            .contains(GoveeCapabilities::RAZER_STREAMING)
    );
    assert_eq!(profile.lan_segment_count, Some(10));
    assert_eq!(profile.razer_led_count, Some(20));
}

#[test]
fn h70b1_uses_openrgb_razer_override_without_lan_segments() {
    let profile = profile_for_sku("H70B1").expect("H70B1 profile should exist");

    assert!(
        profile
            .capabilities
            .contains(GoveeCapabilities::RAZER_STREAMING)
    );
    assert_eq!(profile.lan_segment_count, None);
    assert_eq!(profile.razer_led_count, Some(20));
}

#[test]
fn registry_covers_current_local_capability_table() {
    assert_eq!(known_sku_count(), 266);
    assert_eq!(known_cloud_sku_count(), 259);
    for sku in hypercolor_driver_govee::capabilities::BASIC_LAN_SKUS {
        let profile = profile_for_sku(sku).expect("basic SKU should resolve");
        assert!(
            profile
                .capabilities
                .intersects(GoveeCapabilities::LAN | GoveeCapabilities::CLOUD),
            "{} should have at least one reachable transport",
            profile.sku
        );
    }
    for seed in hypercolor_driver_govee::capabilities::CUSTOM_LAN_PROFILES {
        let profile = profile_for_sku(seed.sku).expect("custom SKU should resolve");
        assert!(
            profile
                .capabilities
                .intersects(GoveeCapabilities::LAN | GoveeCapabilities::CLOUD),
            "{} should have at least one reachable transport",
            profile.sku
        );
    }
}

#[test]
fn registry_covers_current_cloud_lighting_table() {
    for sku in hypercolor_driver_govee::capabilities::CLOUD_SUPPORTED_SKUS {
        let profile = profile_for_sku(sku).expect("cloud SKU should resolve");
        assert!(
            profile.capabilities.contains(GoveeCapabilities::CLOUD),
            "{} should be cloud reachable",
            profile.sku
        );
    }
}

#[test]
fn cloud_only_sku_does_not_claim_lan_transport() {
    let profile = profile_for_sku("H6002").expect("official cloud SKU should resolve");

    assert!(profile.capabilities.contains(GoveeCapabilities::CLOUD));
    assert!(!profile.capabilities.contains(GoveeCapabilities::LAN));
    assert_eq!(profile.lan_segment_count, None);
}
