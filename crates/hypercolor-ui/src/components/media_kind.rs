//! Leptos-free media-kind vocabulary: MIME classification, per-kind Luminary
//! category accent/label/icon, and human-readable size/duration formatting.
//!
//! Kept `crate::`- and leptos-free (icondata + std only) so the contract
//! tests can `#[path]`-include it directly, mirroring
//! `pages/studio/device_grouping.rs`. The leptos card and player components
//! and the catalog page all draw their vocabulary from here so the three
//! cannot drift.

use icondata::{LuCode, LuFilm, LuFolder, LuImage};

/// Coarse media kind from a MIME type, falling back to the filename for the
/// JSON/lottie case: `image` / `gif` / `video` / `lottie` / `other`.
#[must_use]
pub fn kind_from_mime(mime: &str, name: &str) -> &'static str {
    let mime = mime.to_lowercase();
    if mime == "image/gif" {
        "gif"
    } else if mime.starts_with("image/") {
        "image"
    } else if mime.starts_with("video/") {
        "video"
    } else if mime == "application/json" || name.to_lowercase().ends_with(".json") {
        "lottie"
    } else {
        "other"
    }
}

/// Luminary category accent (`R, G, B` triplet) for a media kind, drawn from
/// the DESIGN-SYSTEM §4.1 category color map. Drives the card's `--glow-rgb`,
/// top strip, and identity dot — never interactive chrome, which stays purple.
#[must_use]
pub fn kind_accent(kind: &str) -> &'static str {
    match kind {
        "image" => "128, 255, 234",
        "gif" => "255, 106, 193",
        "video" => "130, 170, 255",
        "lottie" => "80, 250, 123",
        _ => "139, 133, 160",
    }
}

/// Display label for a media kind.
#[must_use]
pub fn kind_label(kind: &str) -> &'static str {
    match kind {
        "image" => "Image",
        "gif" => "GIF",
        "video" => "Video",
        "lottie" => "Lottie",
        _ => "File",
    }
}

/// Lucide icon for a media kind.
#[must_use]
pub fn kind_icon(kind: &str) -> icondata_core::Icon {
    match kind {
        "image" | "gif" => LuImage,
        "video" => LuFilm,
        "lottie" => LuCode,
        _ => LuFolder,
    }
}

/// Whether the daemon generates a raster thumbnail for this kind. Only image
/// families decode to a thumbnail; video and lottie fall back to a placeholder.
#[must_use]
pub fn kind_has_thumbnail(kind: &str) -> bool {
    matches!(kind, "image" | "gif")
}

/// Human-readable byte size (`1.4 MB`).
#[must_use]
pub fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let bytes = bytes as f64;
    if bytes >= GB {
        format!("{:.1} GB", bytes / GB)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes / MB)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes / KB)
    } else {
        format!("{} B", bytes as u64)
    }
}

/// Human-readable clip duration from a microsecond count (`4.2s`, `1m 03s`).
#[must_use]
pub fn format_duration(micros: u64) -> String {
    let seconds = micros as f64 / 1_000_000.0;
    if seconds >= 60.0 {
        let minutes = (seconds / 60.0).floor();
        let remainder = seconds - minutes * 60.0;
        format!("{minutes:.0}m {remainder:02.0}s")
    } else {
        format!("{seconds:.1}s")
    }
}

/// `M:SS` timecode from a seconds value; guards the `NaN`/`Infinity` that a
/// not-yet-loaded media element reports for `duration`.
#[must_use]
pub fn format_timecode(secs: f64) -> String {
    if !secs.is_finite() || secs < 0.0 {
        return "0:00".to_owned();
    }
    let total = secs.floor() as u64;
    format!("{}:{:02}", total / 60, total % 60)
}
