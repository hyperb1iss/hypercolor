//! Tests for screen navigation and identification.

use hypercolor_tui::screen::ScreenId;

#[test]
fn all_screens_returns_six() {
    assert_eq!(ScreenId::all().len(), 6);
}

#[test]
fn default_screen_is_dashboard() {
    assert_eq!(ScreenId::default(), ScreenId::Dashboard);
}

#[test]
fn from_key_lowercase() {
    assert_eq!(ScreenId::from_key('d'), Some(ScreenId::Dashboard));
    assert_eq!(ScreenId::from_key('e'), Some(ScreenId::EffectBrowser));
    assert_eq!(ScreenId::from_key('v'), Some(ScreenId::DeviceManager));
    assert_eq!(ScreenId::from_key('p'), Some(ScreenId::Profiles));
    assert_eq!(ScreenId::from_key('s'), Some(ScreenId::Settings));
    assert_eq!(ScreenId::from_key('b'), Some(ScreenId::Debug));
}

#[test]
fn from_key_uppercase() {
    assert_eq!(ScreenId::from_key('D'), Some(ScreenId::Dashboard));
    assert_eq!(ScreenId::from_key('E'), Some(ScreenId::EffectBrowser));
}

#[test]
fn from_key_unmapped_returns_none() {
    assert_eq!(ScreenId::from_key('x'), None);
    assert_eq!(ScreenId::from_key('q'), None);
    assert_eq!(ScreenId::from_key('1'), None);
    assert_eq!(ScreenId::from_key('c'), None);
}

#[test]
fn key_hint_roundtrip() {
    // Every screen's key_hint should map back to itself via from_key
    for &screen in ScreenId::all() {
        let hint = screen.key_hint();
        let resolved = ScreenId::from_key(hint);
        assert_eq!(
            resolved,
            Some(screen),
            "key_hint '{hint}' for {screen:?} doesn't roundtrip"
        );
    }
}

#[test]
fn labels_are_nonempty() {
    for &screen in ScreenId::all() {
        assert!(!screen.label().is_empty(), "{screen:?} has empty label");
    }
}

#[test]
fn display_shows_label() {
    assert_eq!(format!("{}", ScreenId::Dashboard), "Dash");
    assert_eq!(format!("{}", ScreenId::EffectBrowser), "Effx");
    assert_eq!(format!("{}", ScreenId::Debug), "Dbug");
}

#[test]
fn all_screens_have_unique_keys() {
    let keys: Vec<char> = ScreenId::all().iter().map(|s| s.key_hint()).collect();
    let mut deduped = keys.clone();
    deduped.sort_unstable();
    deduped.dedup();
    assert_eq!(
        keys.len(),
        deduped.len(),
        "duplicate key hints detected: {keys:?}"
    );
}

#[test]
fn screen_id_hash_eq() {
    use std::collections::HashSet;
    let mut set = HashSet::new();
    for &screen in ScreenId::all() {
        assert!(set.insert(screen), "duplicate screen: {screen:?}");
    }
    assert_eq!(set.len(), 6);
}
