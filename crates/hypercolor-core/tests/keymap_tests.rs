//! Cross-platform key naming.
//!
//! The interesting assertion here is totality, not spot checks. Two hand-kept
//! tables drift by someone adding a key to one and forgetting the other, and a
//! curated sample of tuples cannot catch that — so these walk the whole shared
//! inventory in both directions.

use hypercolor_core::input::keymap::{
    CANONICAL_KEYS, KeyNameResult, MEDIA_KEYS, evdev_key_name, scancode_key_name, scancode_name,
};
use hypercolor_windows_input::RawKeyPrefix;

#[test]
fn every_inventory_row_resolves_from_both_key_spaces() {
    for row in CANONICAL_KEYS {
        assert_eq!(
            evdev_key_name(row.evdev_code),
            Some(row.name),
            "evdev code {} does not resolve to {}",
            row.evdev_code,
            row.name
        );
        assert_eq!(
            scancode_name(row.make_code, row.prefix),
            Some(row.name),
            "make code {:#04X} prefix {:?} does not resolve to {}",
            row.make_code,
            row.prefix,
            row.name
        );
    }
}

#[test]
fn every_media_key_resolves_to_the_same_name_on_both_platforms() {
    for row in MEDIA_KEYS {
        assert_eq!(evdev_key_name(row.evdev_code), Some(row.name));
        assert_eq!(
            scancode_key_name(0, RawKeyPrefix::None, row.windows_vkey),
            KeyNameResult::Media(row.name)
        );
        assert_eq!(
            scancode_key_name(0x22, RawKeyPrefix::None, row.windows_vkey),
            KeyNameResult::Media(row.name),
            "media identity wins even when firmware supplies an overlapping scan code"
        );
    }
}

#[test]
fn media_key_identifier_spaces_have_no_duplicates() {
    let mut evdev_codes: Vec<u16> = MEDIA_KEYS.iter().map(|row| row.evdev_code).collect();
    evdev_codes.sort_unstable();
    let before = evdev_codes.len();
    evdev_codes.dedup();
    assert_eq!(before, evdev_codes.len());

    let mut virtual_keys: Vec<u16> = MEDIA_KEYS.iter().map(|row| row.windows_vkey).collect();
    virtual_keys.sort_unstable();
    let before = virtual_keys.len();
    virtual_keys.dedup();
    assert_eq!(before, virtual_keys.len());
}

#[test]
fn the_two_key_spaces_have_no_duplicate_entries() {
    // A duplicate would make one of the two lookups shadow the other, so the
    // tables would silently disagree for exactly one key.
    let mut evdev_codes: Vec<u16> = CANONICAL_KEYS.iter().map(|row| row.evdev_code).collect();
    evdev_codes.sort_unstable();
    let before = evdev_codes.len();
    evdev_codes.dedup();
    assert_eq!(
        before,
        evdev_codes.len(),
        "duplicate evdev code in inventory"
    );

    let mut scancodes: Vec<(u16, RawKeyPrefix)> = CANONICAL_KEYS
        .iter()
        .map(|row| (row.make_code, row.prefix))
        .collect();
    scancodes.sort_unstable();
    let before = scancodes.len();
    scancodes.dedup();
    assert_eq!(
        before,
        scancodes.len(),
        "duplicate (make_code, prefix) in inventory"
    );
}

#[test]
fn names_are_unique_so_two_physical_keys_never_collide() {
    let mut names: Vec<&str> = CANONICAL_KEYS.iter().map(|row| row.name).collect();
    names.sort_unstable();
    let before = names.len();
    names.dedup();
    assert_eq!(before, names.len(), "two rows share a canonical name");
}

#[test]
fn the_inventory_covers_the_keys_positional_effects_depend_on() {
    // wasdVector() and every movement effect are built on these, so a
    // regression that dropped one would be quiet and severe.
    for name in [
        "w",
        "a",
        "s",
        "d",
        "ArrowUp",
        "ArrowDown",
        "ArrowLeft",
        "ArrowRight",
        "Space",
    ] {
        assert!(
            CANONICAL_KEYS.iter().any(|row| row.name == name),
            "{name} is missing from the canonical inventory"
        );
    }
}

#[test]
fn left_and_right_modifiers_are_distinct_positions() {
    // The whole point of positional naming: a layout cannot merge these.
    for (left, right) in [
        ("ControlLeft", "ControlRight"),
        ("ShiftLeft", "ShiftRight"),
        ("AltLeft", "AltRight"),
        ("MetaLeft", "MetaRight"),
    ] {
        let left_row = CANONICAL_KEYS
            .iter()
            .find(|row| row.name == left)
            .unwrap_or_else(|| panic!("{left} missing"));
        let right_row = CANONICAL_KEYS
            .iter()
            .find(|row| row.name == right)
            .unwrap_or_else(|| panic!("{right} missing"));
        assert_ne!(
            (left_row.make_code, left_row.prefix),
            (right_row.make_code, right_row.prefix)
        );
        assert_ne!(left_row.evdev_code, right_row.evdev_code);
    }
}

#[test]
fn the_extended_block_is_separated_only_by_its_prefix() {
    // ControlRight and ControlLeft share scan code 0x1D; only the E0 prefix
    // tells them apart. Dropping the prefix would merge the two modifiers.
    assert_eq!(scancode_name(0x1D, RawKeyPrefix::None), Some("ControlLeft"));
    assert_eq!(scancode_name(0x1D, RawKeyPrefix::E0), Some("ControlRight"));
    assert_eq!(scancode_name(0x1C, RawKeyPrefix::None), Some("Enter"));
    assert_eq!(scancode_name(0x1C, RawKeyPrefix::E0), Some("NumpadEnter"));
}

// ── scancode_key_name ──────────────────────────────────────────────────────

#[test]
fn a_known_scan_code_resolves_positionally() {
    assert_eq!(
        scancode_key_name(0x1E, RawKeyPrefix::None, 0x41),
        KeyNameResult::Positional("a")
    );
}

#[test]
fn the_virtual_key_is_ignored_when_a_position_is_available() {
    // On AZERTY the key in the QWERTY-Q position reports VK_A. Consulting the
    // virtual key here would make a French keyboard name that key "a" and
    // silently break every positional effect.
    assert_eq!(
        scancode_key_name(0x10, RawKeyPrefix::None, 0x41),
        KeyNameResult::Positional("q"),
        "the scan code wins over a layout-translated virtual key"
    );
}

#[test]
fn pause_is_identified_by_its_virtual_key_not_by_e1_alone() {
    const VK_PAUSE: u16 = 0x13;
    assert_eq!(
        scancode_key_name(0x1D, RawKeyPrefix::E1, VK_PAUSE),
        KeyNameResult::Positional("Pause")
    );
}

#[test]
fn an_unrecognized_e1_sequence_is_not_force_fit_to_pause() {
    // Raw Input does not contract that E1 means Pause, so naming every E1
    // report "Pause" would report a key nobody pressed.
    let resolved = scancode_key_name(0x45, RawKeyPrefix::E1, 0x00);
    assert!(
        matches!(resolved, KeyNameResult::Unknown(_)),
        "expected an unknown-key name, got {resolved:?}"
    );
}

#[test]
fn a_zero_scan_code_resolves_through_the_shared_media_inventory() {
    const VK_MEDIA_PLAY_PAUSE: u16 = 0xB3;
    let resolved = scancode_key_name(0, RawKeyPrefix::None, VK_MEDIA_PLAY_PAUSE);
    assert_eq!(resolved, KeyNameResult::Media("MediaPlayPause"));
    assert!(
        !matches!(resolved, KeyNameResult::Positional(_)),
        "a consumer-control key must never be reported as positional"
    );
}

#[test]
fn an_overrun_report_names_nothing() {
    const OVERRUN: u16 = 0xFF;
    assert_eq!(
        scancode_key_name(OVERRUN, RawKeyPrefix::None, 0),
        KeyNameResult::Discard
    );
    assert_eq!(
        scancode_key_name(OVERRUN, RawKeyPrefix::None, 0).name(),
        None
    );
}

#[test]
fn an_unmapped_scan_code_is_named_rather_than_dropped() {
    // Remapping keyboards and quirky firmware emit codes outside set 1, and an
    // effect author can debug a strange name far more easily than a key that
    // does nothing at all.
    let resolved = scancode_key_name(0x76, RawKeyPrefix::None, 0);
    assert!(matches!(resolved, KeyNameResult::Unknown(_)));
    assert!(resolved.name().is_some_and(|name| !name.is_empty()));
}

#[test]
fn unknown_names_are_stable_across_calls() {
    let first = scancode_key_name(0x76, RawKeyPrefix::None, 0).name();
    let second = scancode_key_name(0x76, RawKeyPrefix::None, 0).name();
    assert_eq!(first, second, "held state keys on the name being stable");
}
