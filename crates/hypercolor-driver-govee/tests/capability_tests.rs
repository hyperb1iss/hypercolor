use hypercolor_driver_govee::{GoveeCapabilities, profile_for_sku};

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
fn seeded_profiles_have_a_reachable_transport() {
    for profile in hypercolor_driver_govee::capabilities::SKU_PROFILES {
        assert!(
            profile
                .capabilities
                .intersects(GoveeCapabilities::LAN | GoveeCapabilities::CLOUD),
            "{} should have at least one reachable transport",
            profile.sku
        );
    }
}
