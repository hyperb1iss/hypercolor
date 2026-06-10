//! Tests for the thumbnail store's eviction policy.

use std::collections::HashMap;

use hypercolor_ui::thumbnails::{Thumbnail, ThumbnailPalette, evict_oldest};

fn thumb(captured_at: f64) -> Thumbnail {
    Thumbnail {
        data_url: "data:image/webp;base64,AAAA".to_owned(),
        palette: ThumbnailPalette {
            primary: "225, 53, 255".to_owned(),
            secondary: "128, 255, 234".to_owned(),
            tertiary: "255, 106, 193".to_owned(),
        },
        version: "v1".to_owned(),
        captured_at,
    }
}

fn store_with_captures(captures: &[(&str, f64)]) -> HashMap<String, Thumbnail> {
    captures
        .iter()
        .map(|(effect_id, captured_at)| ((*effect_id).to_owned(), thumb(*captured_at)))
        .collect()
}

#[test]
fn evict_oldest_keeps_store_unchanged_under_cap() {
    let mut map = store_with_captures(&[("a", 1.0), ("b", 2.0), ("c", 3.0)]);
    evict_oldest(&mut map, 3);
    assert_eq!(map.len(), 3);
    assert!(map.contains_key("a"));
    assert!(map.contains_key("b"));
    assert!(map.contains_key("c"));
}

#[test]
fn evict_oldest_removes_only_the_oldest_captures() {
    let mut map = store_with_captures(&[
        ("oldest", 10.0),
        ("older", 20.0),
        ("recent", 30.0),
        ("newest", 40.0),
    ]);
    evict_oldest(&mut map, 2);
    assert_eq!(map.len(), 2);
    assert!(map.contains_key("recent"));
    assert!(map.contains_key("newest"));
}

#[test]
fn evict_oldest_orders_by_captured_at_not_insertion() {
    // Insertion order deliberately scrambled relative to capture time.
    let mut map = store_with_captures(&[("mid", 200.0), ("new", 300.0), ("old", 100.0)]);
    evict_oldest(&mut map, 1);
    assert_eq!(map.len(), 1);
    assert!(map.contains_key("new"));
}

#[test]
fn evict_oldest_with_zero_cap_clears_the_store() {
    let mut map = store_with_captures(&[("a", 1.0), ("b", 2.0)]);
    evict_oldest(&mut map, 0);
    assert!(map.is_empty());
}

#[test]
fn evict_oldest_handles_empty_store() {
    let mut map: HashMap<String, Thumbnail> = HashMap::new();
    evict_oldest(&mut map, 4);
    assert!(map.is_empty());
}
