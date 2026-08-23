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
