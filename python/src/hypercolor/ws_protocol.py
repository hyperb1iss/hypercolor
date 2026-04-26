"""WebSocket protocol constants shared by client helpers."""

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
    "canvas",
    "screen_canvas",
    "web_viewport_canvas",
    "metrics",
    "device_metrics",
    "display_preview",
)
WS_CAPABILITIES: Final = (*WS_CHANNELS, "commands", "canvas_format_jpeg")

BINARY_MESSAGE_TAGS: Final = MappingProxyType(
    {
        "led_frame": 0x01,
        "spectrum": 0x02,
        "canvas": 0x03,
        "screen_canvas": 0x05,
        "web_viewport_canvas": 0x06,
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
