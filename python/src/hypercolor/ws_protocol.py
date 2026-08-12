"""Generated WebSocket protocol constants."""

from __future__ import annotations

from types import MappingProxyType
from typing import Final

WS_PROTOCOL_VERSION: Final = "1.0"
WS_SUBPROTOCOL: Final = "hypercolor-v1"
DEFAULT_WS_SUBSCRIPTIONS: Final = ("events",)

WS_CHANNELS: Final = (
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
    "input_events",
    "commands",
    "canvas_format_jpeg",
    "interactive_previews",
    "wide_preview_frames",
    "preview_chunking",
    "preview_transport_v2:decoded=536870912,encoded=536936448,connection=1073872896,reassembly=8388608,tombstones=4194304,sender=8388608,cursors=8388608,idle_ms=5000,message=1048576",
    "preview_transport_v1:decoded=536870912,encoded=536936448,connection=1073872896,streams=256,tombstones=1024,idle_ms=5000,message=1048576,chunks=4096",
)

JSON_PAYLOAD_CONTRACTS: Final = MappingProxyType(
    {
        "timed_input_event_v1": (
            1,
            "input_events",
            "input_event_received",
        ),
        "input_source_status_changed_v1": (
            1,
            "events",
            "input_source_status_changed",
        ),
        "macos_daemon_ownership_changed_v1": (
            1,
            "events",
            "macos_daemon_ownership_changed",
        ),
    }
)

BINARY_MESSAGE_TAGS: Final = MappingProxyType(
    {
        "led_frame": 0x01,
        "spectrum": 0x02,
        "canvas": 0x03,
        "screen_canvas": 0x05,
        "screen_zones": 0x09,
        "web_viewport_canvas": 0x06,
        "zone_preview": 0x08,
        "display_preview": 0x07,
        "interactive_preview": 0x0A,
        "wide_preview": 0x0B,
        "wide_zone_preview": 0x0C,
        "wide_interactive_preview": 0x0D,
        "wide_screen_zones": 0x0E,
        "preview_chunk": 0x0F,
        "preview_cancel": 0x10,
        "extended_screen_zones": 0x11,
    }
)
PREVIEW_CHANNEL_TAGS: Final = MappingProxyType(
    {
        0x03: "canvas",
        0x05: "screen_canvas",
        0x06: "web_viewport_canvas",
        0x07: "display_preview",
    }
)
CANVAS_FORMAT_TAGS: Final = MappingProxyType(
    {
        0: "rgb",
        1: "rgba",
        2: "jpeg",
    }
)
