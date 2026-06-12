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
    "commands",
    "canvas_format_jpeg",
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
