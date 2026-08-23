"""Generated WebSocket protocol constants."""

from __future__ import annotations

from types import MappingProxyType
from typing import Final

WS_PROTOCOL_VERSION: Final = "1.0"
WS_SUBPROTOCOL: Final = "hypercolor-v1"
DEFAULT_WS_SUBSCRIPTIONS: Final = ("events",)

WS_TOPICS: Final = (
    "frames",
    "spectrum",
    "events",
    "frame_events",
    "canvas",
    "screen_canvas",
    "screen_zones",
    "web_viewport_canvas",
    "zone_preview",
    "metrics",
    "device_metrics",
    "sensors",
    "display_preview",
    "interactive_preview",
    "input_events",
)
WS_CAPABILITIES: Final = (
    "frames",
    "spectrum",
    "events",
    "frame_events",
    "canvas",
    "screen_canvas",
    "screen_zones",
    "web_viewport_canvas",
    "zone_preview",
    "metrics",
    "device_metrics",
    "sensors",
    "display_preview",
    "interactive_preview",
    "input_events",
    "commands",
    "canvas_format_jpeg",
    "interactive_previews",
    "wide_preview_frames",
    "preview_chunking",
)

PREVIEW_TRANSPORT: Final = MappingProxyType(
    {
        "dimension_type": "u32",
        "passive_zero_dimension": "auto",
        "interactive_zero_dimension": "invalid",
        "max_publication_decoded_bytes": 536870912,
        "max_publication_encoded_bytes": 536936448,
        "max_connection_bytes": 1073872896,
        "partial_idle_ms": 5000,
        "max_message_bytes": 1048576,
        "max_reassembly_state_bytes": 8388608,
        "max_tombstone_bytes": 4194304,
        "max_sender_state_bytes": 8388608,
        "max_cursor_state_bytes": 8388608,
        "min_message_bytes": 184,
        "jpeg_max_axis": 65535,
    }
)

BINARY_FRAME_LAYOUTS: Final = MappingProxyType(
    {
        "display_preview_frame": MappingProxyType(
            {
                "prefix_len": 15,
                "offsets": MappingProxyType(
                    {
                        "tag": 0,
                        "device_id_len": 1,
                        "frame_number": 2,
                        "timestamp_ms": 6,
                        "width": 10,
                        "height": 12,
                        "format": 14,
                        "device_id": 15,
                    }
                ),
                "types": MappingProxyType(
                    {
                        "tag": "u8",
                        "device_id_len": "u8",
                        "frame_number": "u32_le",
                        "timestamp_ms": "u32_le",
                        "width": "u16_le",
                        "height": "u16_le",
                        "format": "u8",
                        "device_id": "utf8",
                    }
                ),
                "formats": MappingProxyType(
                    {
                        "rgb": 0,
                        "rgba": 1,
                        "jpeg": 2,
                    }
                ),
            }
        ),
        "extended_screen_zones_frame": MappingProxyType(
            {
                "prefix_len": 41,
                "offsets": MappingProxyType(
                    {
                        "tag": 0,
                        "frame_number": 1,
                        "timestamp_ms": 5,
                        "source_width": 9,
                        "source_height": 13,
                        "grid_cols": 17,
                        "grid_rows": 21,
                        "letterbox_top": 25,
                        "letterbox_bottom": 29,
                        "letterbox_left": 33,
                        "letterbox_right": 37,
                        "zone_colors": 41,
                    }
                ),
                "types": MappingProxyType(
                    {
                        "tag": "u8",
                        "frame_number": "u32_le",
                        "timestamp_ms": "u32_le",
                        "source_width": "u32_le",
                        "source_height": "u32_le",
                        "grid_cols": "u32_le",
                        "grid_rows": "u32_le",
                        "letterbox_top": "u32_le",
                        "letterbox_bottom": "u32_le",
                        "letterbox_left": "u32_le",
                        "letterbox_right": "u32_le",
                        "zone_colors": "repeated_u8_rgb",
                    }
                ),
            }
        ),
        "interactive_preview_frame": MappingProxyType(
            {
                "prefix_len": 15,
                "offsets": MappingProxyType(
                    {
                        "tag": 0,
                        "preview_id_len": 1,
                        "frame_number": 2,
                        "timestamp_ms": 6,
                        "width": 10,
                        "height": 12,
                        "format": 14,
                        "preview_id": 15,
                    }
                ),
                "types": MappingProxyType(
                    {
                        "tag": "u8",
                        "preview_id_len": "u8",
                        "frame_number": "u32_le",
                        "timestamp_ms": "u32_le",
                        "width": "u16_le",
                        "height": "u16_le",
                        "format": "u8",
                        "preview_id": "utf8",
                    }
                ),
                "formats": MappingProxyType(
                    {
                        "rgb": 0,
                        "rgba": 1,
                        "jpeg": 2,
                    }
                ),
            }
        ),
        "preview_cancel_frame": MappingProxyType(
            {
                "prefix_len": 14,
                "offsets": MappingProxyType(
                    {
                        "tag": 0,
                        "schema": 1,
                        "stream_kind": 2,
                        "channel_tag": 3,
                        "stream_identity_len": 4,
                        "publication_id": 6,
                        "stream_identity": 14,
                    }
                ),
                "types": MappingProxyType(
                    {
                        "tag": "u8",
                        "schema": "u8",
                        "stream_kind": "u8",
                        "channel_tag": "u8",
                        "stream_identity_len": "u16_le",
                        "publication_id": "u64_le",
                        "stream_identity": "bytes",
                    }
                ),
            }
        ),
        "preview_chunk_frame": MappingProxyType(
            {
                "prefix_len": 55,
                "offsets": MappingProxyType(
                    {
                        "tag": 0,
                        "schema": 1,
                        "stream_kind": 2,
                        "channel_tag": 3,
                        "format": 4,
                        "stream_identity_len": 5,
                        "publication_id": 7,
                        "frame_number": 15,
                        "timestamp_ms": 19,
                        "width": 23,
                        "height": 27,
                        "total_encoded_bytes": 31,
                        "chunk_offset": 39,
                        "chunk_index": 47,
                        "chunk_count": 51,
                        "stream_identity": 55,
                    }
                ),
                "types": MappingProxyType(
                    {
                        "tag": "u8",
                        "schema": "u8",
                        "stream_kind": "u8",
                        "channel_tag": "u8",
                        "format": "u8",
                        "stream_identity_len": "u16_le",
                        "publication_id": "u64_le",
                        "frame_number": "u32_le",
                        "timestamp_ms": "u32_le",
                        "width": "u32_le",
                        "height": "u32_le",
                        "total_encoded_bytes": "u64_le",
                        "chunk_offset": "u64_le",
                        "chunk_index": "u32_le",
                        "chunk_count": "u32_le",
                        "stream_identity": "bytes",
                    }
                ),
            }
        ),
        "preview_frame": MappingProxyType(
            {
                "prefix_len": 14,
                "offsets": MappingProxyType(
                    {
                        "tag": 0,
                        "frame_number": 1,
                        "timestamp_ms": 5,
                        "width": 9,
                        "height": 11,
                        "format": 13,
                        "payload": 14,
                    }
                ),
                "types": MappingProxyType(
                    {
                        "tag": "u8",
                        "frame_number": "u32_le",
                        "timestamp_ms": "u32_le",
                        "width": "u16_le",
                        "height": "u16_le",
                        "format": "u8",
                        "payload": "bytes",
                    }
                ),
                "formats": MappingProxyType(
                    {
                        "rgb": 0,
                        "rgba": 1,
                        "jpeg": 2,
                    }
                ),
            }
        ),
        "screen_zones_frame": MappingProxyType(
            {
                "prefix_len": 19,
                "offsets": MappingProxyType(
                    {
                        "tag": 0,
                        "frame_number": 1,
                        "timestamp_ms": 5,
                        "source_width": 9,
                        "source_height": 11,
                        "grid_cols": 13,
                        "grid_rows": 14,
                        "letterbox_top": 15,
                        "letterbox_bottom": 16,
                        "letterbox_left": 17,
                        "letterbox_right": 18,
                        "zone_colors": 19,
                    }
                ),
                "types": MappingProxyType(
                    {
                        "tag": "u8",
                        "frame_number": "u32_le",
                        "timestamp_ms": "u32_le",
                        "source_width": "u16_le",
                        "source_height": "u16_le",
                        "grid_cols": "u8",
                        "grid_rows": "u8",
                        "letterbox_top": "u8",
                        "letterbox_bottom": "u8",
                        "letterbox_left": "u8",
                        "letterbox_right": "u8",
                        "zone_colors": "repeated_u8_rgb",
                    }
                ),
            }
        ),
        "wide_display_preview_frame": MappingProxyType(
            {
                "prefix_len": 19,
                "offsets": MappingProxyType(
                    {
                        "tag": 0,
                        "device_id_len": 1,
                        "frame_number": 2,
                        "timestamp_ms": 6,
                        "width": 10,
                        "height": 14,
                        "format": 18,
                        "device_id": 19,
                    }
                ),
                "types": MappingProxyType(
                    {
                        "tag": "u8",
                        "device_id_len": "u8",
                        "frame_number": "u32_le",
                        "timestamp_ms": "u32_le",
                        "width": "u32_le",
                        "height": "u32_le",
                        "format": "u8",
                        "device_id": "utf8",
                    }
                ),
            }
        ),
        "wide_interactive_preview_frame": MappingProxyType(
            {
                "prefix_len": 19,
                "offsets": MappingProxyType(
                    {
                        "tag": 0,
                        "preview_id_len": 1,
                        "frame_number": 2,
                        "timestamp_ms": 6,
                        "width": 10,
                        "height": 14,
                        "format": 18,
                        "preview_id": 19,
                    }
                ),
                "types": MappingProxyType(
                    {
                        "tag": "u8",
                        "preview_id_len": "u8",
                        "frame_number": "u32_le",
                        "timestamp_ms": "u32_le",
                        "width": "u32_le",
                        "height": "u32_le",
                        "format": "u8",
                        "preview_id": "utf8",
                    }
                ),
            }
        ),
        "wide_preview_frame": MappingProxyType(
            {
                "prefix_len": 19,
                "offsets": MappingProxyType(
                    {
                        "tag": 0,
                        "channel_tag": 1,
                        "frame_number": 2,
                        "timestamp_ms": 6,
                        "width": 10,
                        "height": 14,
                        "format": 18,
                        "payload": 19,
                    }
                ),
                "types": MappingProxyType(
                    {
                        "tag": "u8",
                        "channel_tag": "u8",
                        "frame_number": "u32_le",
                        "timestamp_ms": "u32_le",
                        "width": "u32_le",
                        "height": "u32_le",
                        "format": "u8",
                        "payload": "bytes",
                    }
                ),
            }
        ),
        "wide_screen_zones_frame": MappingProxyType(
            {
                "prefix_len": 23,
                "offsets": MappingProxyType(
                    {
                        "tag": 0,
                        "frame_number": 1,
                        "timestamp_ms": 5,
                        "source_width": 9,
                        "source_height": 13,
                        "grid_cols": 17,
                        "grid_rows": 18,
                        "letterbox_top": 19,
                        "letterbox_bottom": 20,
                        "letterbox_left": 21,
                        "letterbox_right": 22,
                        "zone_colors": 23,
                    }
                ),
                "types": MappingProxyType(
                    {
                        "tag": "u8",
                        "frame_number": "u32_le",
                        "timestamp_ms": "u32_le",
                        "source_width": "u32_le",
                        "source_height": "u32_le",
                        "grid_cols": "u8",
                        "grid_rows": "u8",
                        "letterbox_top": "u8",
                        "letterbox_bottom": "u8",
                        "letterbox_left": "u8",
                        "letterbox_right": "u8",
                        "zone_colors": "repeated_u8_rgb",
                    }
                ),
            }
        ),
        "wide_zone_preview_frame": MappingProxyType(
            {
                "prefix_len": 50,
                "offsets": MappingProxyType(
                    {
                        "tag": 0,
                        "frame_number": 1,
                        "timestamp_ms": 5,
                        "scene_id": 9,
                        "zone_id": 25,
                        "width": 41,
                        "height": 45,
                        "format": 49,
                        "payload": 50,
                    }
                ),
                "types": MappingProxyType(
                    {
                        "tag": "u8",
                        "frame_number": "u32_le",
                        "timestamp_ms": "u32_le",
                        "scene_id": "uuid",
                        "zone_id": "uuid",
                        "width": "u32_le",
                        "height": "u32_le",
                        "format": "u8",
                        "payload": "bytes",
                    }
                ),
            }
        ),
        "zone_preview_frame": MappingProxyType(
            {
                "prefix_len": 46,
                "offsets": MappingProxyType(
                    {
                        "tag": 0,
                        "frame_number": 1,
                        "timestamp_ms": 5,
                        "scene_id": 9,
                        "zone_id": 25,
                        "width": 41,
                        "height": 43,
                        "format": 45,
                        "payload": 46,
                    }
                ),
                "types": MappingProxyType(
                    {
                        "tag": "u8",
                        "frame_number": "u32_le",
                        "timestamp_ms": "u32_le",
                        "scene_id": "uuid",
                        "zone_id": "uuid",
                        "width": "u16_le",
                        "height": "u16_le",
                        "format": "u8",
                        "payload": "bytes",
                    }
                ),
                "formats": MappingProxyType(
                    {
                        "rgb": 0,
                        "rgba": 1,
                        "jpeg": 2,
                    }
                ),
            }
        ),
    }
)

BINARY_MESSAGE_LAYOUTS: Final = MappingProxyType(
    {
        "led_frame": MappingProxyType(
            {
                "prefix_len": 11,
                "offsets": MappingProxyType(
                    {
                        "tag": 0,
                        "frame_number": 1,
                        "timestamp_ms": 5,
                        "zone_count": 9,
                        "zones": 11,
                    }
                ),
                "types": MappingProxyType(
                    {
                        "tag": "u8",
                        "frame_number": "u32_le",
                        "timestamp_ms": "u32_le",
                        "zone_count": "u16_le",
                        "zones": "repeated_zone",
                    }
                ),
            }
        ),
        "spectrum": MappingProxyType(
            {
                "prefix_len": 27,
                "offsets": MappingProxyType(
                    {
                        "tag": 0,
                        "timestamp_ms": 1,
                        "bin_count": 5,
                        "level": 6,
                        "bass": 10,
                        "mid": 14,
                        "treble": 18,
                        "beat": 22,
                        "beat_confidence": 23,
                        "bins": 27,
                    }
                ),
                "types": MappingProxyType(
                    {
                        "tag": "u8",
                        "timestamp_ms": "u32_le",
                        "bin_count": "u8",
                        "level": "f32_le",
                        "bass": "f32_le",
                        "mid": "f32_le",
                        "treble": "f32_le",
                        "beat": "u8",
                        "beat_confidence": "f32_le",
                        "bins": "repeated_f32_le",
                    }
                ),
            }
        ),
    }
)

JSON_PAYLOAD_CONTRACTS: Final = MappingProxyType(
    {
        "timed_input_event_v1": MappingProxyType(
            {
                "schema_version": 1,
                "event": "input_event_received",
                "required_fields": (
                    "event",
                    "at_ms",
                    "seq",
                    "repeat_count",
                ),
                "optional_fields": MappingProxyType(
                    {
                        "physical_code": None,
                    }
                ),
                "description": "Canonical captured input edge with exact timing, ordering, and repeat multiplicity.",
                "topic": "input_events",
            }
        ),
        "input_source_status_changed_v1": MappingProxyType(
            {
                "schema_version": 1,
                "event": "input_source_status_changed",
                "required_fields": (
                    "source_id",
                    "kind",
                    "backend",
                    "configured",
                    "consented",
                    "demanded",
                    "active_consumer_count",
                    "state",
                    "freshness",
                    "source_graph_generation",
                    "session_generation",
                    "resource_count",
                    "denied_resource_count",
                    "retired",
                ),
                "optional_fields": MappingProxyType(
                    {
                        "lifecycle_issue_code": None,
                        "freshness_issue_code": None,
                    }
                ),
                "description": "Coalesced input-source lifecycle and freshness transition. Contains operational metadata only and never captured input contents.",
                "topic": "events",
            }
        ),
        "macos_daemon_ownership_changed_v1": MappingProxyType(
            {
                "schema_version": 1,
                "topic": "events",
                "event": "macos_daemon_ownership_changed",
                "required_fields": (
                    "active_owner",
                    "owner_epoch",
                ),
                "optional_fields": MappingProxyType(
                    {
                        "conflict": None,
                        "recovery_required": None,
                    }
                ),
                "description": "Authoritative macOS daemon topology snapshot. The event reports ownership state only and cannot request an owner change.",
            }
        ),
    }
)

BINARY_MESSAGE_TAGS: Final = MappingProxyType(
    {
        "led_frame": 0x01,
        "spectrum": 0x02,
        "canvas": 0x03,
        "screen_canvas": 0x05,
        "web_viewport_canvas": 0x06,
        "display_preview": 0x07,
        "zone_preview": 0x08,
        "screen_zones": 0x09,
        "interactive_preview": 0x0A,
        "wide_preview": 0x0B,
        "wide_zone_preview": 0x0C,
        "wide_interactive_preview": 0x0D,
        "wide_screen_zones": 0x0E,
        "preview_chunk": 0x0F,
        "preview_cancel": 0x10,
        "extended_screen_zones": 0x11,
        "wide_display_preview": 0x12,
    }
)
PREVIEW_TOPIC_TAGS: Final = MappingProxyType(
    {
        0x03: "canvas",
        0x05: "screen_canvas",
        0x06: "web_viewport_canvas",
    }
)
CANVAS_FORMAT_TAGS: Final = MappingProxyType(
    {
        0: "rgb",
        1: "rgba",
        2: "jpeg",
    }
)
