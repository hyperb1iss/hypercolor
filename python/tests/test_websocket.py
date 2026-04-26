"""Tests for the Hypercolor WebSocket helpers."""

from __future__ import annotations

import struct

import pytest

from hypercolor.websocket import (
    CanvasData,
    EventMessage,
    FrameData,
    HelloMessage,
    HypercolorEventStream,
    SpectrumData,
)


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
    assert message.height == 2
    assert message.pixels == pixels


def test_unknown_json_message_falls_back_to_event() -> None:
    message = HypercolorEventStream._decode_json('{"type":"subscribed","channels":["events"]}')

    assert isinstance(message, EventMessage)
    assert message.event == "subscribed"
