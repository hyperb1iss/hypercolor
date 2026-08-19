use std::sync::Arc;

use hypercolor_types::host_input::{
    HOST_KEYS, HOST_MEDIA_KEYS, HostInputEvent, HostKeyIdentity, HostKeySignal, HostPointerButton,
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
fn every_physical_row_resolves_from_each_platform_space() {
    for row in HOST_KEYS {
        assert_eq!(host_key_name_from_evdev(row.evdev_code), Some(row.name));
        assert_eq!(
            host_key_name_from_windows(row.windows_make_code, row.windows_prefix),
            Some(row.name)
        );
        assert_eq!(
            host_key_name_from_macos(row.macos_virtual_keycode),
            Some(row.name)
        );
    }
}

#[test]
fn media_inventory_is_unique_and_resolves_where_supported() {
    for (index, row) in HOST_MEDIA_KEYS.iter().enumerate() {
        assert_eq!(host_key_name_from_evdev(row.evdev_code), Some(row.name));
        assert_eq!(
            host_media_key_name_from_windows(row.windows_vkey),
            Some(row.name)
        );
        if let Some(nx_key_type) = row.macos_nx_key_type {
            assert_eq!(host_media_key_name_from_macos(nx_key_type), Some(row.name));
        }
        for other in &HOST_MEDIA_KEYS[index + 1..] {
            assert_ne!(row.evdev_code, other.evdev_code);
            assert_ne!(row.windows_vkey, other.windows_vkey);
            if let (Some(row_type), Some(other_type)) =
                (row.macos_nx_key_type, other.macos_nx_key_type)
            {
                assert_ne!(row_type, other_type);
            }
            assert_ne!(row.name, other.name);
        }
    }
}

#[test]
fn canonical_inventory_preserves_distinct_modifier_positions() {
    for (left, right) in [
        ("ControlLeft", "ControlRight"),
        ("ShiftLeft", "ShiftRight"),
        ("AltLeft", "AltRight"),
        ("MetaLeft", "MetaRight"),
    ] {
        let left = HOST_KEYS
            .iter()
            .find(|row| row.name == left)
            .expect("left modifier belongs to the canonical inventory");
        let right = HOST_KEYS
            .iter()
            .find(|row| row.name == right)
            .expect("right modifier belongs to the canonical inventory");
        assert_ne!(left.evdev_code, right.evdev_code);
        assert_ne!(
            (left.windows_make_code, left.windows_prefix),
            (right.windows_make_code, right.windows_prefix)
        );
        assert_ne!(left.macos_virtual_keycode, right.macos_virtual_keycode);
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
