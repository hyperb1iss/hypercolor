use std::sync::Arc;

use hypercolor_types::host_input::{
    HOST_KEYS, HostInputEvent, HostKeyIdentity, HostKeySignal, HostPointerButton,
    HostRepeatEvidence, HostScanCodePrefix, host_key_name_from_evdev, host_key_name_from_macos,
    host_key_name_from_windows, host_media_key_name_from_macos, host_media_key_name_from_windows,
};

#[test]
fn canonical_inventory_resolves_primitive_platform_codes() {
    assert_eq!(host_key_name_from_evdev(30), Some("a"));
    assert_eq!(
        host_key_name_from_windows(0x1d, HostScanCodePrefix::E0),
        Some("ControlRight")
    );
    assert_eq!(host_key_name_from_macos(0x7e), Some("ArrowUp"));
    assert_eq!(
        host_media_key_name_from_windows(0xb3),
        Some("MediaPlayPause")
    );
    assert_eq!(host_media_key_name_from_macos(17), Some("MediaTrackNext"));
    assert_eq!(host_key_name_from_evdev(u16::MAX), None);
}

#[test]
fn physical_inventory_has_unique_platform_identifiers() {
    for (index, row) in HOST_KEYS.iter().enumerate() {
        for other in &HOST_KEYS[index + 1..] {
            assert_ne!(row.evdev_code, other.evdev_code);
            assert_ne!(
                (row.windows_make_code, row.windows_prefix),
                (other.windows_make_code, other.windows_prefix)
            );
            assert_ne!(row.macos_virtual_keycode, other.macos_virtual_keycode);
            assert_ne!(row.name, other.name);
        }
    }
}

#[test]
fn neutral_vocabulary_accepts_uninventoried_names() {
    let identity = HostKeyIdentity {
        key: Arc::from("VendorMacro42"),
        physical_code: Arc::from("vendor:0x42"),
    };
    let event = HostInputEvent::Key {
        device: None,
        identity: identity.clone(),
        signal: HostKeySignal::Edge {
            pressed: true,
            repeat: HostRepeatEvidence::Unknown,
        },
    };

    assert_eq!(HostPointerButton::new("button_42").as_str(), "button_42");
    assert!(matches!(
        event,
        HostInputEvent::Key {
            identity: actual,
            ..
        } if actual == identity
    ));
}

#[test]
fn aggregate_modifier_evidence_preserves_counterpart() {
    let signal = HostKeySignal::AggregateState {
        active: true,
        active_counterpart: Some(Arc::from("ShiftRight")),
    };

    assert!(matches!(
        signal,
        HostKeySignal::AggregateState {
            active: true,
            active_counterpart: Some(counterpart),
        } if &*counterpart == "ShiftRight"
    ));
}
