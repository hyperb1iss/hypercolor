#![allow(dead_code, unused_imports)]

#[path = "../src/ws/mod.rs"]
mod ws;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PreviewRenderOutcome {
    Presented,
    Reinitialize,
}

#[path = "../src/components/preview_runtime/canvas2d.rs"]
mod canvas2d;

use canvas2d::expand_rgb_to_rgba_bytes;

#[test]
fn expand_rgb_to_rgba_bytes_adds_opaque_alpha() {
    let mut rgba = Vec::new();
    expand_rgb_to_rgba_bytes(&[0x10, 0x20, 0x30, 0xaa, 0xbb, 0xcc], &mut rgba);
    assert_eq!(rgba, vec![0x10, 0x20, 0x30, 0xff, 0xaa, 0xbb, 0xcc, 0xff]);
}

#[test]
fn expand_rgb_to_rgba_bytes_reuses_destination_storage() {
    let mut rgba = vec![0; 16];
    let capacity_before = rgba.capacity();
    expand_rgb_to_rgba_bytes(&[0x01, 0x02, 0x03], &mut rgba);
    assert_eq!(rgba, vec![0x01, 0x02, 0x03, 0xff]);
    assert_eq!(rgba.capacity(), capacity_before);
}
