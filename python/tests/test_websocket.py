"""Tests for the Hypercolor WebSocket helpers."""

from __future__ import annotations

import struct
from pathlib import Path
from typing import Any

import msgspec
import pytest

from hypercolor import ws_protocol
from hypercolor.websocket import (
    CanvasData,
    EventMessage,
    FrameData,
    HelloMessage,
    HypercolorEventStream,
    SpectrumData,
)

PROTOCOL_MANIFEST = Path(__file__).resolve().parents[2] / "protocol" / "websocket-v1.json"


def test_ws_protocol_constants_match_manifest() -> None:
    manifest = msgspec.json.decode(PROTOCOL_MANIFEST.read_bytes())
    assert isinstance(manifest, dict)

    channels = _expect_list(manifest["channels"])
    binary_messages = _expect_list(manifest["binary_messages"])
    preview_formats = _expect_dict(_expect_dict(manifest["preview_frame"])["formats"])

    assert manifest["version"] == ws_protocol.WS_PROTOCOL_VERSION
    assert manifest["subprotocol"] == ws_protocol.WS_SUBPROTOCOL
    assert list(ws_protocol.WS_CHANNELS) == [str(channel["name"]) for channel in channels]
    assert list(ws_protocol.WS_CAPABILITIES) == _expect_list(manifest["capabilities"])
    assert dict(ws_protocol.BINARY_MESSAGE_TAGS) == {
        str(message["name"]): int(message["tag"]) for message in binary_messages
    }
    assert dict(ws_protocol.PREVIEW_CHANNEL_TAGS) == {
        int(message["tag"]): str(message["channel"])
        for message in binary_messages
        if message["layout"] == "preview_frame"
    }
    assert dict(ws_protocol.CANVAS_FORMAT_TAGS) == {
        int(tag): name for name, tag in preview_formats.items()
    }


def test_decode_hello_message() -> None:
    message = HypercolorEventStream._decode_json(
        '{"type":"hello","version":"1.0","state":{"running":true},"capabilities":["events"],"subscriptions":["events"]}'
    )

    assert isinstance(message, HelloMessage)
    assert message.version == "1.0"
    assert message.capabilities == ["events"]


def test_parse_led_frame() -> None:
    zone_id = b"zone_0"
    rgb = bytes([255, 0, 255, 0, 255, 255])
    payload = bytearray()
    payload.extend(b"\x01")
    payload.extend(struct.pack("<II", 7, 1234))
    payload.extend(b"\x01")
    payload.extend(struct.pack("<H", len(zone_id)))
    payload.extend(zone_id)
    payload.extend(struct.pack("<H", 2))
    payload.extend(rgb)

    message = HypercolorEventStream._parse_led_frame(bytes(payload))

    assert isinstance(message, FrameData)
    assert message.frame_number == 7
    assert message.zones[0].zone_id == "zone_0"
    assert message.zones[0].rgb == rgb


def test_parse_spectrum() -> None:
    payload = bytearray()
    payload.extend(b"\x02")
    payload.extend(struct.pack("<I", 4321))
    payload.extend(b"\x02")
    payload.extend(struct.pack("<ffff", 0.5, 0.6, 0.4, 0.2))
    payload.extend(b"\x01")
    payload.extend(struct.pack("<f", 0.75))
    payload.extend(struct.pack("<2f", 0.1, 0.9))

    message = HypercolorEventStream._parse_spectrum(bytes(payload))

    assert isinstance(message, SpectrumData)
    assert message.beat is True
    assert message.bins == pytest.approx([0.1, 0.9])


def test_parse_canvas() -> None:
    pixels = b"\x00\x11\x22\x33\x44\x55"
    payload = bytearray()
    payload.extend(b"\x03")
    payload.extend(struct.pack("<II", 5, 999))
    payload.extend(struct.pack("<HH", 1, 2))
    payload.extend(b"\x00")
    payload.extend(pixels)

    message = HypercolorEventStream._parse_canvas(bytes(payload))

    assert isinstance(message, CanvasData)
    assert message.format == "rgb"
    assert message.channel == "canvas"
    assert message.height == 2
    assert message.pixels == pixels


def test_parse_display_preview_jpeg() -> None:
    jpeg = b"\xff\xd8\xff\xe0preview"
    payload = bytearray()
    payload.extend(b"\x07")
    payload.extend(struct.pack("<II", 8, 1001))
    payload.extend(struct.pack("<HH", 64, 32))
    payload.extend(b"\x02")
    payload.extend(jpeg)

    message = HypercolorEventStream._parse_canvas(bytes(payload))

    assert isinstance(message, CanvasData)
    assert message.channel == "display_preview"
    assert message.format == "jpeg"
    assert message.width == 64
    assert message.pixels == jpeg


def test_unknown_json_message_falls_back_to_event() -> None:
    message = HypercolorEventStream._decode_json('{"type":"subscribed","channels":["events"]}')

    assert isinstance(message, EventMessage)
    assert message.event == "subscribed"


def _expect_dict(value: Any) -> dict[str, Any]:
    assert isinstance(value, dict)
    return value


def _expect_list(value: Any) -> list[Any]:
    assert isinstance(value, list)
    return value
