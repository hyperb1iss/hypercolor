//! Contract tests for the leptos-free media-kind vocabulary.
//!
//! `media_kind.rs` is leptos- and `crate::`-free (icondata + std only) so it
//! can be pulled in directly, mirroring `pages/studio/device_grouping.rs`.

// The path-included module carries view-only helpers (`kind_icon`) the
// formatting/classification tests don't exercise.
#![allow(dead_code)]

#[path = "../src/components/media_kind.rs"]
mod media_kind;

use media_kind::{
    format_bytes, format_duration, format_timecode, kind_accent, kind_from_mime,
    kind_has_thumbnail, kind_label,
};

#[test]
fn kind_from_mime_classifies_each_family() {
    assert_eq!(kind_from_mime("image/png", "a.png"), "image");
    assert_eq!(kind_from_mime("image/jpeg", "a.jpg"), "image");
    assert_eq!(kind_from_mime("image/webp", "a.webp"), "image");
    // GIF is its own kind, distinct from still images.
    assert_eq!(kind_from_mime("image/gif", "a.gif"), "gif");
    assert_eq!(kind_from_mime("video/mp4", "a.mp4"), "video");
    assert_eq!(kind_from_mime("video/webm", "a.webm"), "video");
    assert_eq!(kind_from_mime("application/json", "a.json"), "lottie");
    assert_eq!(kind_from_mime("application/octet-stream", "x.bin"), "other");
}

#[test]
fn kind_from_mime_falls_back_to_filename_for_lottie() {
    // A lottie uploaded without a JSON mime still resolves by extension.
    assert_eq!(
        kind_from_mime("application/octet-stream", "spinner.json"),
        "lottie"
    );
    assert_eq!(kind_from_mime("", "ANIMATION.JSON"), "lottie");
}

#[test]
fn kind_from_mime_is_case_insensitive() {
    assert_eq!(kind_from_mime("IMAGE/PNG", "A.PNG"), "image");
    assert_eq!(kind_from_mime("Video/MP4", "A.MP4"), "video");
}

#[test]
fn kind_accent_matches_category_color_map() {
    assert_eq!(kind_accent("image"), "128, 255, 234");
    assert_eq!(kind_accent("gif"), "255, 106, 193");
    assert_eq!(kind_accent("video"), "130, 170, 255");
    assert_eq!(kind_accent("lottie"), "80, 250, 123");
    assert_eq!(kind_accent("other"), "139, 133, 160");
    assert_eq!(kind_accent("unknown-kind"), "139, 133, 160");
}

#[test]
fn kind_label_is_human_readable() {
    assert_eq!(kind_label("image"), "Image");
    assert_eq!(kind_label("gif"), "GIF");
    assert_eq!(kind_label("video"), "Video");
    assert_eq!(kind_label("lottie"), "Lottie");
    assert_eq!(kind_label("other"), "File");
}

#[test]
fn only_image_families_have_thumbnails() {
    assert!(kind_has_thumbnail("image"));
    assert!(kind_has_thumbnail("gif"));
    // The daemon cannot decode these, so the card draws a placeholder.
    assert!(!kind_has_thumbnail("video"));
    assert!(!kind_has_thumbnail("lottie"));
    assert!(!kind_has_thumbnail("other"));
}

#[test]
fn format_bytes_scales_units() {
    assert_eq!(format_bytes(512), "512 B");
    assert_eq!(format_bytes(1024), "1.0 KB");
    assert_eq!(format_bytes(1_572_864), "1.5 MB");
    assert_eq!(format_bytes(3_221_225_472), "3.0 GB");
}

#[test]
fn format_duration_switches_to_minutes_past_a_minute() {
    assert_eq!(format_duration(4_200_000), "4.2s");
    assert_eq!(format_duration(63_000_000), "1m 03s");
    assert_eq!(format_duration(0), "0.0s");
}

#[test]
fn format_timecode_formats_minutes_and_seconds() {
    assert_eq!(format_timecode(0.0), "0:00");
    assert_eq!(format_timecode(9.0), "0:09");
    assert_eq!(format_timecode(65.4), "1:05");
    assert_eq!(format_timecode(600.0), "10:00");
}

#[test]
fn format_timecode_guards_unloaded_media_duration() {
    // A media element reports NaN/Infinity for `duration` before metadata
    // loads, and seeking can momentarily produce a negative — all clamp to 0.
    assert_eq!(format_timecode(f64::NAN), "0:00");
    assert_eq!(format_timecode(f64::INFINITY), "0:00");
    assert_eq!(format_timecode(-3.0), "0:00");
}
