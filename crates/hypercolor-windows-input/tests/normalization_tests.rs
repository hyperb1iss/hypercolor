use std::sync::Arc;

use hypercolor_types::host_input::{
    HostInputCapabilities, HostInputDevice, HostInputEvent, HostKeySignal, HostRepeatEvidence,
};
use hypercolor_windows_input::decode::{
    CanonicalKeyReport, KEYBOARD_OVERRUN_MAKE_CODE, KeyCanonicalizer,
};
use hypercolor_windows_input::{RawKeyPrefix, normalize_windows_key_event};

fn keyboard() -> Arc<HostInputDevice> {
    Arc::new(HostInputDevice {
        source_id: Arc::from("windows:fixture:keyboard"),
        label: Arc::from("fixture keyboard"),
        capabilities: HostInputCapabilities {
            keyboard: true,
            pointer: false,
        },
        session_generation: 3,
        device_generation: 7,
    })
}

fn normalized_key(make_code: u16, prefix: RawKeyPrefix, vkey: u16) -> (Arc<str>, Arc<str>) {
    let HostInputEvent::Key {
        identity, signal, ..
    } = normalize_windows_key_event(&keyboard(), make_code, prefix, vkey, true)
    else {
        panic!("expected key event");
    };
    assert_eq!(
        signal,
        HostKeySignal::Edge {
            pressed: true,
            repeat: HostRepeatEvidence::Unknown,
        }
    );
    (identity.key, identity.physical_code)
}

#[test]
fn positional_keys_use_the_shared_canonical_inventory() {
    assert_eq!(
        normalized_key(0x1E, RawKeyPrefix::None, 0x41),
        (Arc::from("a"), Arc::from("windows:set1:none:1e"))
    );
}

#[test]
fn media_keys_use_the_shared_consumer_control_inventory() {
    assert_eq!(
        normalized_key(0, RawKeyPrefix::None, 0xAF).0.as_ref(),
        "AudioVolumeUp"
    );
}

#[test]
fn pause_preserves_its_e1_identity() {
    assert_eq!(
        normalized_key(0x1D, RawKeyPrefix::E1, 0x13),
        (Arc::from("Pause"), Arc::from("windows:set1:e1:1d"))
    );
}

#[test]
fn logical_only_keys_use_the_virtual_key_fallback() {
    assert_eq!(
        normalized_key(0, RawKeyPrefix::None, 0xA6).0.as_ref(),
        "BrowserBack"
    );
}

#[test]
fn unknown_positions_are_stable_and_prefix_qualified() {
    assert_eq!(
        normalized_key(0x7F, RawKeyPrefix::E0, 0).0.as_ref(),
        "UnknownE0007F"
    );
}

#[test]
fn rollover_overrun_is_rejected_before_normalization() {
    assert_eq!(
        KeyCanonicalizer.canonicalize(KEYBOARD_OVERRUN_MAKE_CODE, 0, 0),
        CanonicalKeyReport::Overrun
    );
}
