use serde_json::{Map, Value, json};

use super::preview::{
    DISPLAY_PREVIEW_FRAME_PREFIX_LEN, EXTENDED_SCREEN_ZONES_FRAME_HEADER_LEN,
    INTERACTIVE_PREVIEW_FRAME_PREFIX_LEN, PREVIEW_CANCEL_FIXED_HEADER_LEN, PREVIEW_CANCEL_SCHEMA,
    PREVIEW_CHUNK_FIXED_HEADER_LEN, PREVIEW_CHUNK_SCHEMA, PREVIEW_FRAME_HEADER_LEN,
    PreviewPixelFormat, SCREEN_ZONES_FRAME_HEADER_LEN, WIDE_DISPLAY_PREVIEW_FRAME_PREFIX_LEN,
    WIDE_INTERACTIVE_PREVIEW_FRAME_PREFIX_LEN, WIDE_PREVIEW_FRAME_HEADER_LEN,
    WIDE_SCREEN_ZONES_FRAME_HEADER_LEN, WIDE_ZONE_PREVIEW_FRAME_HEADER_LEN,
    ZONE_PREVIEW_FRAME_HEADER_LEN,
};
use super::spectrum::SPECTRUM_FRAME_HEADER_LEN;

fn fields(layout: &[(&str, &str)]) -> Value {
    Value::Array(
        layout
            .iter()
            .map(|(encoding, name)| json!([encoding, name]))
            .collect(),
    )
}

fn formats() -> Value {
    json!({
        "rgb": PreviewPixelFormat::Rgb.tag(),
        "rgba": PreviewPixelFormat::Rgba.tag(),
        "jpeg": PreviewPixelFormat::Jpeg.tag(),
    })
}

/// Binary message names and the codec layout each tag uses.
#[must_use]
pub fn codec_binary_messages() -> Map<String, Value> {
    [
        (
            "spectrum",
            json!({
                "header_len": SPECTRUM_FRAME_HEADER_LEN,
                "layout": fields(&[
                    ("u8", "tag"),
                    ("u32_le", "timestamp_ms"),
                    ("u8", "bin_count"),
                    ("f32_le", "level"),
                    ("f32_le", "bass"),
                    ("f32_le", "mid"),
                    ("f32_le", "treble"),
                    ("u8", "beat"),
                    ("f32_le", "beat_confidence"),
                    ("repeated_f32_le", "bins"),
                ]),
            }),
        ),
        ("canvas", json!("preview_frame")),
        ("screen_canvas", json!("preview_frame")),
        ("screen_zones", json!("screen_zones_frame")),
        ("web_viewport_canvas", json!("preview_frame")),
        ("zone_preview", json!("zone_preview_frame")),
        ("display_preview", json!("display_preview_frame")),
        ("interactive_preview", json!("interactive_preview_frame")),
        ("wide_preview", json!("wide_preview_frame")),
        ("wide_zone_preview", json!("wide_zone_preview_frame")),
        (
            "wide_interactive_preview",
            json!("wide_interactive_preview_frame"),
        ),
        ("wide_screen_zones", json!("wide_screen_zones_frame")),
        ("preview_chunk", json!("preview_chunk_frame")),
        ("preview_cancel", json!("preview_cancel_frame")),
        (
            "extended_screen_zones",
            json!("extended_screen_zones_frame"),
        ),
        ("wide_display_preview", json!("wide_display_preview_frame")),
    ]
    .into_iter()
    .map(|(name, layout)| (name.to_owned(), layout))
    .collect()
}

/// Named frame layouts emitted by the preview and spectrum codecs.
#[must_use]
pub fn codec_frame_layouts() -> Map<String, Value> {
    let mut layouts = Map::new();
    layouts.insert(
        "preview_frame".to_owned(),
        json!({
            "header_len": PREVIEW_FRAME_HEADER_LEN,
            "layout": fields(&[
                ("u8", "tag"),
                ("u32_le", "frame_number"),
                ("u32_le", "timestamp_ms"),
                ("u16_le", "width"),
                ("u16_le", "height"),
                ("u8", "format"),
                ("bytes", "payload"),
            ]),
            "formats": formats(),
        }),
    );
    layouts.insert(
        "zone_preview_frame".to_owned(),
        json!({
            "header_len": ZONE_PREVIEW_FRAME_HEADER_LEN,
            "layout": fields(&[
                ("u8", "tag"),
                ("u32_le", "frame_number"),
                ("u32_le", "timestamp_ms"),
                ("uuid", "scene_id"),
                ("uuid", "zone_id"),
                ("u16_le", "width"),
                ("u16_le", "height"),
                ("u8", "format"),
                ("bytes", "payload"),
            ]),
            "formats": formats(),
        }),
    );
    layouts.insert(
        "interactive_preview_frame".to_owned(),
        json!({
            "prefix_len": INTERACTIVE_PREVIEW_FRAME_PREFIX_LEN,
            "layout": fields(&[
                ("u8", "tag"),
                ("u8", "preview_id_len"),
                ("u32_le", "frame_number"),
                ("u32_le", "timestamp_ms"),
                ("u16_le", "width"),
                ("u16_le", "height"),
                ("u8", "format"),
                ("utf8", "preview_id"),
                ("bytes", "payload"),
            ]),
            "formats": formats(),
        }),
    );
    layouts.insert(
        "screen_zones_frame".to_owned(),
        json!({
            "header_len": SCREEN_ZONES_FRAME_HEADER_LEN,
            "layout": fields(&[
                ("u8", "tag"),
                ("u32_le", "frame_number"),
                ("u32_le", "timestamp_ms"),
                ("u16_le", "source_width"),
                ("u16_le", "source_height"),
                ("u8", "grid_cols"),
                ("u8", "grid_rows"),
                ("u8", "letterbox_top"),
                ("u8", "letterbox_bottom"),
                ("u8", "letterbox_left"),
                ("u8", "letterbox_right"),
                ("repeated_u8_rgb", "zone_colors"),
            ]),
        }),
    );
    layouts.insert(
        "wide_preview_frame".to_owned(),
        json!({
            "header_len": WIDE_PREVIEW_FRAME_HEADER_LEN,
            "description": "Additive preview layout for u32 dimensions. The channel_tag identifies the logical passive preview stream.",
            "layout": fields(&[
                ("u8", "tag"),
                ("u8", "channel_tag"),
                ("u32_le", "frame_number"),
                ("u32_le", "timestamp_ms"),
                ("u32_le", "width"),
                ("u32_le", "height"),
                ("u8", "format"),
                ("bytes", "payload"),
            ]),
        }),
    );
    layouts.insert(
        "wide_zone_preview_frame".to_owned(),
        json!({
            "header_len": WIDE_ZONE_PREVIEW_FRAME_HEADER_LEN,
            "layout": fields(&[
                ("u8", "tag"),
                ("u32_le", "frame_number"),
                ("u32_le", "timestamp_ms"),
                ("uuid", "scene_id"),
                ("uuid", "zone_id"),
                ("u32_le", "width"),
                ("u32_le", "height"),
                ("u8", "format"),
                ("bytes", "payload"),
            ]),
        }),
    );
    layouts.insert(
        "wide_interactive_preview_frame".to_owned(),
        json!({
            "prefix_len": WIDE_INTERACTIVE_PREVIEW_FRAME_PREFIX_LEN,
            "layout": fields(&[
                ("u8", "tag"),
                ("u8", "preview_id_len"),
                ("u32_le", "frame_number"),
                ("u32_le", "timestamp_ms"),
                ("u32_le", "width"),
                ("u32_le", "height"),
                ("u8", "format"),
                ("utf8", "preview_id"),
                ("bytes", "payload"),
            ]),
        }),
    );
    layouts.insert(
        "wide_screen_zones_frame".to_owned(),
        json!({
            "header_len": WIDE_SCREEN_ZONES_FRAME_HEADER_LEN,
            "layout": fields(&[
                ("u8", "tag"),
                ("u32_le", "frame_number"),
                ("u32_le", "timestamp_ms"),
                ("u32_le", "source_width"),
                ("u32_le", "source_height"),
                ("u8", "grid_cols"),
                ("u8", "grid_rows"),
                ("u8", "letterbox_top"),
                ("u8", "letterbox_bottom"),
                ("u8", "letterbox_left"),
                ("u8", "letterbox_right"),
                ("repeated_u8_rgb", "zone_colors"),
            ]),
        }),
    );
    layouts.insert(
        "extended_screen_zones_frame".to_owned(),
        json!({
            "header_len": EXTENDED_SCREEN_ZONES_FRAME_HEADER_LEN,
            "layout": fields(&[
                ("u8", "tag"),
                ("u32_le", "frame_number"),
                ("u32_le", "timestamp_ms"),
                ("u32_le", "source_width"),
                ("u32_le", "source_height"),
                ("u32_le", "grid_cols"),
                ("u32_le", "grid_rows"),
                ("u32_le", "letterbox_top"),
                ("u32_le", "letterbox_bottom"),
                ("u32_le", "letterbox_left"),
                ("u32_le", "letterbox_right"),
                ("repeated_u8_rgb", "zone_colors"),
            ]),
        }),
    );
    layouts.insert(
        "preview_chunk_frame".to_owned(),
        json!({
            "schema": PREVIEW_CHUNK_SCHEMA,
            "fixed_header_len": PREVIEW_CHUNK_FIXED_HEADER_LEN,
            "description": "Bounded chunk envelope around one fully encoded compact or wide preview publication.",
            "layout": fields(&[
                ("u8", "tag"),
                ("u8", "schema"),
                ("u8", "stream_kind"),
                ("u8", "channel_tag"),
                ("u8", "format"),
                ("u16_le", "stream_identity_len"),
                ("u64_le", "publication_id"),
                ("u32_le", "frame_number"),
                ("u32_le", "timestamp_ms"),
                ("u32_le", "width"),
                ("u32_le", "height"),
                ("u64_le", "total_encoded_bytes"),
                ("u64_le", "chunk_offset"),
                ("u32_le", "chunk_index"),
                ("u32_le", "chunk_count"),
                ("bytes", "stream_identity"),
                ("bytes", "chunk_payload"),
            ]),
        }),
    );
    layouts.insert(
        "preview_cancel_frame".to_owned(),
        json!({
            "schema": PREVIEW_CANCEL_SCHEMA,
            "fixed_header_len": PREVIEW_CANCEL_FIXED_HEADER_LEN,
            "description": "Publication-specific cancellation preserving per-stream high-water state.",
            "layout": fields(&[
                ("u8", "tag"),
                ("u8", "schema"),
                ("u8", "stream_kind"),
                ("u8", "channel_tag"),
                ("u16_le", "stream_identity_len"),
                ("u64_le", "publication_id"),
                ("bytes", "stream_identity"),
            ]),
        }),
    );
    layouts.insert(
        "display_preview_frame".to_owned(),
        json!({
            "prefix_len": DISPLAY_PREVIEW_FRAME_PREFIX_LEN,
            "layout": fields(&[
                ("u8", "tag"),
                ("u8", "device_id_len"),
                ("u32_le", "frame_number"),
                ("u32_le", "timestamp_ms"),
                ("u16_le", "width"),
                ("u16_le", "height"),
                ("u8", "format"),
                ("utf8", "device_id"),
                ("bytes", "payload"),
            ]),
            "formats": formats(),
        }),
    );
    layouts.insert(
        "wide_display_preview_frame".to_owned(),
        json!({
            "prefix_len": WIDE_DISPLAY_PREVIEW_FRAME_PREFIX_LEN,
            "layout": fields(&[
                ("u8", "tag"),
                ("u8", "device_id_len"),
                ("u32_le", "frame_number"),
                ("u32_le", "timestamp_ms"),
                ("u32_le", "width"),
                ("u32_le", "height"),
                ("u8", "format"),
                ("utf8", "device_id"),
                ("bytes", "payload"),
            ]),
        }),
    );
    layouts
}
