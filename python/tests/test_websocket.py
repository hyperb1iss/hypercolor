"""Tests for the Hypercolor WebSocket helpers."""

from __future__ import annotations

import ast
import asyncio
import runpy
import struct
import uuid
from collections.abc import Callable, Mapping
from pathlib import Path
from types import MappingProxyType
from typing import Any, cast

import msgspec
import pytest

from hypercolor import websocket as websocket_module, ws_protocol
from hypercolor.websocket import (
    ActiveSubscription,
    BinaryMessage,
    CanvasData,
    DisplayPreviewData,
    FrameData,
    HelloMessage,
    HypercolorEventStream,
    InteractivePreviewData,
    ScreenZonesData,
    SpectrumData,
    SubscribedMessage,
    UnsubscribedMessage,
    ZonePreviewData,
    _encode_text,
)

PROTOCOL_MANIFEST = Path(__file__).resolve().parents[2] / "protocol" / "websocket-v1.json"
WS_GENERATOR = Path(__file__).resolve().parents[1] / "scripts" / "generate_ws_protocol.py"


class _TestClient:
    ws_url = "ws://127.0.0.1:9420/api/v1/ws"
    api_key = None


class _ClosableConnection:
    def __init__(self) -> None:
        self.closed = False

    async def close(self) -> None:
        self.closed = True


def test_ws_protocol_constants_match_manifest() -> None:
    manifest = msgspec.json.decode(PROTOCOL_MANIFEST.read_bytes())
    assert isinstance(manifest, dict)

    topics = _expect_list(manifest["topics"])
    binary_messages = _expect_list(manifest["binary_messages"])
    preview_formats = _expect_dict(_expect_dict(manifest["preview_frame"])["formats"])

    assert manifest["version"] == ws_protocol.WS_PROTOCOL_VERSION
    assert manifest["subprotocol"] == ws_protocol.WS_SUBPROTOCOL
    assert list(ws_protocol.WS_TOPICS) == [str(topic["name"]) for topic in topics]
    assert list(ws_protocol.WS_CAPABILITIES) == _expect_list(manifest["capabilities"])
    assert _thaw_json(ws_protocol.JSON_PAYLOAD_CONTRACTS) == _expect_dict(
        manifest["json_payloads"]
    )
    assert dict(ws_protocol.BINARY_MESSAGE_TAGS) == {
        str(message["name"]): int(message["tag"]) for message in binary_messages
    }
    assert dict(ws_protocol.PREVIEW_TOPIC_TAGS) == {
        int(message["tag"]): str(message["topic"])
        for message in binary_messages
        if message["layout"] == "preview_frame"
    }
    assert dict(ws_protocol.CANVAS_FORMAT_TAGS) == {
        int(tag): name for name, tag in preview_formats.items()
    }


def test_ws_protocol_field_defaults_distinguish_absent_from_null() -> None:
    timed_input = _expect_mapping(ws_protocol.JSON_PAYLOAD_CONTRACTS["timed_input_event_v1"])
    required_fields = _expect_tuple(timed_input["required_fields"])
    optional_fields = _expect_mapping(timed_input["optional_fields"])

    assert "event" in required_fields
    assert "event" not in optional_fields
    assert "physical_code" in optional_fields
    assert optional_fields["physical_code"] is None


def test_ws_protocol_contracts_are_deeply_immutable() -> None:
    timed_input = ws_protocol.JSON_PAYLOAD_CONTRACTS["timed_input_event_v1"]
    required_fields = timed_input["required_fields"]
    optional_fields = timed_input["optional_fields"]

    assert isinstance(timed_input, MappingProxyType)
    assert isinstance(required_fields, tuple)
    assert isinstance(optional_fields, MappingProxyType)
    with pytest.raises(TypeError):
        cast(dict[str, Any], optional_fields)["at_ms"] = 99


def test_ws_protocol_generator_round_trips_non_bmp_strings() -> None:
    value = "😀"
    quote = cast(Callable[[str], str], runpy.run_path(str(WS_GENERATOR))["quote"])

    assert ast.literal_eval(quote(value)) == value


def test_preview_transport_negotiates_the_v2_byte_ledger() -> None:
    assert websocket_module._PREVIEW_TRANSPORT_CAPABILITY.startswith("preview_transport_v2:")
    assert websocket_module._PREVIEW_TRANSPORT_LIMITS["reassembly"] > 0
    assert websocket_module._PREVIEW_TRANSPORT_LIMITS["sender"] > 0
    assert "chunks" not in websocket_module._PREVIEW_TRANSPORT_LIMITS


@pytest.mark.asyncio
async def test_subscribe_advertises_preview_transport_v2(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    sent: list[dict[str, Any]] = []

    async def capture(_stream: HypercolorEventStream, payload: dict[str, Any]) -> None:
        sent.append(payload)

    async def acknowledge(
        _stream: HypercolorEventStream,
        _expected: type[SubscribedMessage] | type[UnsubscribedMessage],
    ) -> SubscribedMessage:
        return SubscribedMessage([], websocket_module._PREVIEW_TRANSPORT_CAPABILITY)

    monkeypatch.setattr(HypercolorEventStream, "_send_json", capture)
    monkeypatch.setattr(HypercolorEventStream, "_wait_for_subscription_ack", acknowledge)
    stream = HypercolorEventStream(_TestClient())

    acknowledgment = await stream.subscribe("screen_zones")

    assert sent[0]["preview_transport"] == websocket_module._PREVIEW_TRANSPORT_CAPABILITY
    assert sent[0]["preview_transport"].startswith("preview_transport_v2:")
    assert acknowledgment.preview_transport == websocket_module._PREVIEW_TRANSPORT_CAPABILITY


@pytest.mark.asyncio
async def test_preview_reconfiguration_does_not_renegotiate_transport(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    sent: list[dict[str, Any]] = []

    async def capture(_stream: HypercolorEventStream, payload: dict[str, Any]) -> None:
        sent.append(payload)

    async def acknowledge(
        _stream: HypercolorEventStream,
        _expected: type[SubscribedMessage] | type[UnsubscribedMessage],
    ) -> SubscribedMessage:
        return SubscribedMessage([], websocket_module._PREVIEW_TRANSPORT_CAPABILITY)

    monkeypatch.setattr(HypercolorEventStream, "_send_json", capture)
    monkeypatch.setattr(HypercolorEventStream, "_wait_for_subscription_ack", acknowledge)
    stream = HypercolorEventStream(_TestClient())

    await stream.open_interactive_preview("main", fps=30, width=320, height=180)
    await stream.open_interactive_preview("main", fps=15, width=640, height=360)
    await stream.subscribe("canvas")

    assert sent[0]["preview_transport"] == websocket_module._PREVIEW_TRANSPORT_CAPABILITY
    assert "preview_transport" not in sent[1]
    assert "preview_transport" not in sent[2]


def test_decode_hello_message() -> None:
    message = HypercolorEventStream._decode_json(
        '{"type":"hello","version":"1.0","state":{"running":true},'
        '"capabilities":["events"],'
        '"subscriptions":[{"topic":"events"},'
        '{"topic":"display_preview","key":"device-abc","config":{"fps":15}}]}'
    )

    assert isinstance(message, HelloMessage)
    assert message.version == "1.0"
    assert message.capabilities == ["events"]
    assert message.subscriptions == [
        ActiveSubscription(topic="events"),
        ActiveSubscription(topic="display_preview", key="device-abc", config={"fps": 15}),
    ]


def test_parse_led_frame() -> None:
    zone_id = b"zone_0"
    rgb = bytes([255, 0, 255, 0, 255, 255])
    payload = bytearray()
    payload.extend(b"\x01")
    payload.extend(struct.pack("<II", 7, 1234))
    payload.extend(struct.pack("<H", 1))
    payload.extend(struct.pack("<H", len(zone_id)))
    payload.extend(zone_id)
    payload.extend(struct.pack("<H", 2))
    payload.extend(rgb)

    message = HypercolorEventStream._parse_led_frame(bytes(payload))

    assert isinstance(message, FrameData)
    assert message.frame_number == 7
    assert message.zones[0].zone_id == "zone_0"
    assert message.zones[0].rgb == rgb


def test_parse_led_frame_preserves_zone_count_above_u8() -> None:
    payload = bytearray(b"\x01")
    payload.extend(struct.pack("<IIH", 8, 1235, 257))
    for index in range(257):
        zone_id = f"zone_{index}".encode()
        payload.extend(struct.pack("<H", len(zone_id)))
        payload.extend(zone_id)
        payload.extend(struct.pack("<H", 1))
        payload.extend(bytes((index % 256, 0, 255)))

    message = HypercolorEventStream._parse_led_frame(bytes(payload))

    assert len(message.zones) == 257
    assert message.zones[256].zone_id == "zone_256"
    assert message.zones[256].rgb == b"\x00\x00\xff"


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


def test_parse_keyed_display_preview_jpeg() -> None:
    jpeg = b"\xff\xd8\xff\xe0preview"
    device_id = b"device-abc"
    payload = bytearray((0x07, len(device_id)))
    payload.extend(struct.pack("<II", 8, 1001))
    payload.extend(struct.pack("<HH", 64, 32))
    payload.extend(b"\x02")
    payload.extend(device_id)
    payload.extend(jpeg)

    message = HypercolorEventStream._decode_binary(bytes(payload))

    assert isinstance(message, DisplayPreviewData)
    assert message.device_id == "device-abc"
    assert message.frame_number == 8
    assert message.format == "jpeg"
    assert message.width == 64
    assert message.height == 32
    assert message.pixels == jpeg


def test_parse_wide_display_preview() -> None:
    pixels = bytes(range(12))
    device_id = b"device-abc"
    payload = bytearray((0x12, len(device_id)))
    payload.extend(struct.pack("<II", 3, 4))
    payload.extend(struct.pack("<II", 4, 1))
    payload.extend(b"\x00")
    payload.extend(device_id)
    payload.extend(pixels)

    message = HypercolorEventStream._decode_binary(bytes(payload))

    assert isinstance(message, DisplayPreviewData)
    assert message.device_id == "device-abc"
    assert message.width == 4
    assert message.height == 1
    assert message.format == "rgb"
    assert message.pixels == pixels


def test_parse_wide_interactive_preview() -> None:
    pixels = bytes(range(12))
    preview_id = b"main"
    payload = bytearray((0x0D, len(preview_id)))
    payload.extend(struct.pack("<II", 5, 6))
    payload.extend(struct.pack("<II", 4, 1))
    payload.extend(b"\x00")
    payload.extend(preview_id)
    payload.extend(pixels)

    message = HypercolorEventStream._decode_binary(bytes(payload))

    assert isinstance(message, InteractivePreviewData)
    assert message.preview_id == "main"
    assert message.width == 4
    assert message.height == 1
    assert message.pixels == pixels


def test_parse_addressed_interactive_preview() -> None:
    jpeg = b"\xff\xd8\xff\xe0preview"
    preview_id = b"main"
    payload = bytearray((0x0A, len(preview_id)))
    payload.extend(struct.pack("<II", 9, 1002))
    payload.extend(struct.pack("<HH", 640, 480))
    payload.extend(b"\x02")
    payload.extend(preview_id)
    payload.extend(jpeg)

    message = HypercolorEventStream._decode_binary(bytes(payload))

    assert isinstance(message, InteractivePreviewData)
    assert message.preview_id == "main"
    assert message.frame_number == 9
    assert message.width == 640
    assert message.height == 480
    assert message.format == "jpeg"
    assert message.pixels == jpeg


@pytest.mark.parametrize("payload", [b"\x0a", b"\x0a\x04" + b"\x00" * 13])
def test_interactive_preview_rejects_truncated_frames(payload: bytes) -> None:
    with pytest.raises(ValueError, match=r"shorter|truncated"):
        HypercolorEventStream._decode_binary(payload)


@pytest.mark.parametrize(
    ("preview_id", "match"),
    [(b"", "empty"), (b"a" * 129, "exceeds"), (b"bad\nname", "control")],
)
def test_interactive_preview_rejects_invalid_ids(preview_id: bytes, match: str) -> None:
    payload = bytearray((0x0A, len(preview_id)))
    payload.extend(struct.pack("<IIHHB", 1, 2, 1, 1, 2))
    payload.extend(preview_id)
    payload.extend(b"jpeg")
    with pytest.raises(ValueError, match=match):
        HypercolorEventStream._decode_binary(bytes(payload))


@pytest.mark.parametrize(("format_byte", "pixels"), [(0, b"\x00" * 5), (1, b"\x00" * 7)])
def test_interactive_preview_rejects_truncated_raw_payloads(
    format_byte: int, pixels: bytes
) -> None:
    preview_id = b"main"
    payload = bytearray((0x0A, len(preview_id)))
    payload.extend(struct.pack("<IIHHB", 1, 2, 2, 1, format_byte))
    payload.extend(preview_id)
    payload.extend(pixels)
    with pytest.raises(ValueError, match="payload is too short"):
        HypercolorEventStream._decode_binary(bytes(payload))


def test_subscribed_ack_is_a_distinct_public_message() -> None:
    message = HypercolorEventStream._decode_json(
        '{"type":"subscribed","preview_transport":"preview_transport_v1",'
        '"topics":[{"topic":"events"}]}'
    )

    assert isinstance(message, SubscribedMessage)
    assert message.topics == [ActiveSubscription(topic="events")]
    assert message.preview_transport == "preview_transport_v1"


def _expect_dict(value: Any) -> dict[str, Any]:
    assert isinstance(value, dict)
    return value


def _expect_list(value: Any) -> list[Any]:
    assert isinstance(value, list)
    return value


def _expect_mapping(value: Any) -> Mapping[str, Any]:
    assert isinstance(value, Mapping)
    return value


def _expect_tuple(value: Any) -> tuple[Any, ...]:
    assert isinstance(value, tuple)
    return value


def _thaw_json(value: Any) -> Any:
    if isinstance(value, Mapping):
        return {key: _thaw_json(child) for key, child in value.items()}
    if isinstance(value, tuple):
        return [_thaw_json(child) for child in value]
    return value


def test_parse_zone_preview() -> None:
    scene_id = uuid.uuid4()
    zone_id = uuid.uuid4()
    pixels = b"\x01\x02\x03"
    payload = bytearray()
    payload.extend(b"\x08")
    payload.extend(struct.pack("<II", 42, 4242))
    payload.extend(scene_id.bytes)
    payload.extend(zone_id.bytes)
    payload.extend(struct.pack("<HH", 1, 1))
    payload.extend(b"\x00")
    payload.extend(pixels)

    message = HypercolorEventStream._decode_binary(bytes(payload))

    assert isinstance(message, ZonePreviewData)
    assert message.scene_id == str(scene_id)
    assert message.zone_id == str(zone_id)
    assert message.format == "rgb"
    assert message.pixels == pixels


def test_parse_screen_zones() -> None:
    rgb = bytes([255, 0, 255] * 6)
    payload = bytearray()
    payload.extend(b"\x09")
    payload.extend(struct.pack("<II", 9, 9999))
    payload.extend(struct.pack("<HH", 1920, 1080))
    payload.extend(bytes([3, 2]))
    payload.extend(bytes([0, 1, 0, 0]))
    payload.extend(rgb)

    message = HypercolorEventStream._decode_binary(bytes(payload))

    assert isinstance(message, ScreenZonesData)
    assert message.grid_cols == 3
    assert message.grid_rows == 2
    assert message.letterbox == (0, 1, 0, 0)
    assert message.rgb == rgb


def test_parse_extended_screen_zones() -> None:
    payload = (
        struct.pack(
            "<B10I",
            ws_protocol.BINARY_MESSAGE_TAGS["extended_screen_zones"],
            43,
            9877,
            100_000,
            2160,
            256,
            1,
            0,
            0,
            256,
            0,
        )
        + bytes([1, 2, 3]) * 256
    )

    message = HypercolorEventStream._decode_binary(payload)

    assert isinstance(message, ScreenZonesData)
    assert message.frame_number == 43
    assert message.source_width == 100_000
    assert message.grid_cols == 256
    assert message.grid_rows == 1
    assert message.letterbox == (0, 0, 256, 0)
    assert message.rgb == bytes([1, 2, 3]) * 256


def test_parse_preserved_wide_source_screen_zones() -> None:
    payload = struct.pack(
        "<B4I6B",
        ws_protocol.BINARY_MESSAGE_TAGS["wide_screen_zones"],
        44,
        9878,
        100_000,
        2160,
        2,
        1,
        0,
        0,
        1,
        0,
    ) + bytes([1, 2, 3, 4, 5, 6])

    message = HypercolorEventStream._decode_binary(payload)

    assert isinstance(message, ScreenZonesData)
    assert message.frame_number == 44
    assert message.source_width == 100_000
    assert message.grid_cols == 2
    assert message.letterbox == (0, 0, 1, 0)
    assert message.rgb == bytes([1, 2, 3, 4, 5, 6])


def test_parse_screen_zones_rejects_wrong_payload_length() -> None:
    payload = struct.pack(
        "<BIIHH6B",
        ws_protocol.BINARY_MESSAGE_TAGS["screen_zones"],
        42,
        9876,
        1920,
        1080,
        2,
        1,
        0,
        0,
        0,
        0,
    ) + bytes([1, 2, 3])

    with pytest.raises(ValueError, match="must be 6 bytes"):
        HypercolorEventStream._decode_binary(payload)


@pytest.mark.parametrize(
    "payload",
    [
        bytes([ws_protocol.BINARY_MESSAGE_TAGS["screen_zones"]]),
        bytes([ws_protocol.BINARY_MESSAGE_TAGS["wide_screen_zones"]]) + bytes(21),
        bytes([ws_protocol.BINARY_MESSAGE_TAGS["extended_screen_zones"]]) + bytes(39),
    ],
)
def test_parse_screen_zones_rejects_truncated_headers(payload: bytes) -> None:
    with pytest.raises(ValueError, match="shorter than"):
        HypercolorEventStream._decode_binary(payload)


@pytest.mark.asyncio
async def test_screen_zones_dispatches_to_registered_handlers() -> None:
    stream = HypercolorEventStream(_TestClient())
    received: list[ScreenZonesData] = []
    stream.on_screen_zones(received.append)
    message = ScreenZonesData(
        frame_number=1,
        timestamp_ms=2,
        source_width=3840,
        source_height=2160,
        grid_cols=256,
        grid_rows=1,
        letterbox=(0, 0, 0, 0),
        rgb=bytes(256 * 3),
    )

    await stream._dispatch_binary(message)

    assert received == [message]


def _preview_chunks(
    encoded: bytes,
    publication_id: int,
    chunk_bytes: int,
    *,
    stream_kind: int,
    channel_tag: int,
    identity: bytes,
    frame_number: int,
    timestamp_ms: int,
    width: int,
    height: int,
    pixel_format: int = 0,
) -> list[bytes]:
    chunk_count = (len(encoded) + chunk_bytes - 1) // chunk_bytes
    chunks = []
    for chunk_index in range(chunk_count):
        offset = chunk_index * chunk_bytes
        chunk = encoded[offset : offset + chunk_bytes]
        header = struct.pack(
            "<5BHQ4I2Q2I",
            ws_protocol.BINARY_MESSAGE_TAGS["preview_chunk"],
            1,
            stream_kind,
            channel_tag,
            pixel_format,
            len(identity),
            publication_id,
            frame_number,
            timestamp_ms,
            width,
            height,
            len(encoded),
            offset,
            chunk_index,
            chunk_count,
        )
        chunks.append(header + identity + chunk)
    return chunks


def _screen_zone_chunks(encoded: bytes, publication_id: int, chunk_bytes: int) -> list[bytes]:
    frame_number, timestamp_ms = struct.unpack_from("<II", encoded, 1)
    if encoded[0] == ws_protocol.BINARY_MESSAGE_TAGS["screen_zones"]:
        source_width, source_height = struct.unpack_from("<HH", encoded, 9)
    else:
        source_width, source_height = struct.unpack_from("<II", encoded, 9)
    return _preview_chunks(
        encoded,
        publication_id,
        chunk_bytes,
        stream_kind=3,
        channel_tag=ws_protocol.BINARY_MESSAGE_TAGS["screen_zones"],
        identity=b"",
        frame_number=frame_number,
        timestamp_ms=timestamp_ms,
        width=source_width,
        height=source_height,
    )


def _preview_cancel(
    publication_id: int, *, stream_kind: int, channel_tag: int, identity: bytes
) -> bytes:
    return (
        struct.pack(
            "<4BHQ",
            ws_protocol.BINARY_MESSAGE_TAGS["preview_cancel"],
            1,
            stream_kind,
            channel_tag,
            len(identity),
            publication_id,
        )
        + identity
    )


def _screen_zone_cancel(publication_id: int) -> bytes:
    return _preview_cancel(
        publication_id,
        stream_kind=3,
        channel_tag=ws_protocol.BINARY_MESSAGE_TAGS["screen_zones"],
        identity=b"",
    )


def _extended_screen_zones_frame(grid_cols: int, grid_rows: int, rgb: bytes) -> bytes:
    return (
        struct.pack(
            "<B10I",
            ws_protocol.BINARY_MESSAGE_TAGS["extended_screen_zones"],
            1,
            2,
            3840,
            2160,
            grid_cols,
            grid_rows,
            0,
            0,
            0,
            0,
        )
        + rgb
    )


def test_screen_zone_chunk_payload_borrows_the_websocket_frame() -> None:
    encoded = _extended_screen_zones_frame(4, 1, bytes(12))
    frame = _screen_zone_chunks(encoded, 99, 30)[0]

    chunk = websocket_module._parse_screen_zone_chunk(frame)

    assert isinstance(chunk.payload, memoryview)
    assert chunk.payload.obj is frame


@pytest.mark.asyncio
async def test_chunked_extended_screen_zones_reassemble_and_dispatch() -> None:
    grid_cols = 350_000
    rgb = bytes([1, 2, 3]) * grid_cols
    encoded = (
        struct.pack(
            "<B10I",
            ws_protocol.BINARY_MESSAGE_TAGS["extended_screen_zones"],
            45,
            9879,
            3840,
            2160,
            grid_cols,
            1,
            0,
            0,
            0,
            0,
        )
        + rgb
    )
    chunks = _screen_zone_chunks(encoded, 9001, 600_000)
    stream = HypercolorEventStream(_TestClient())
    received: list[ScreenZonesData] = []
    stream.on_screen_zones(received.append)

    first = stream._decode_received_binary(chunks[0])
    assert isinstance(first, BinaryMessage)
    completed = stream._decode_received_binary(chunks[1])
    await stream._dispatch_binary(completed)

    assert len(received) == 1
    assert received[0].grid_cols == grid_cols
    assert received[0].rgb == rgb
    assert stream._screen_zones_reassembler.reserved_bytes == 0


def test_screen_zone_chunks_reject_out_of_order_and_duplicate_data() -> None:
    encoded = struct.pack(
        "<B10I",
        ws_protocol.BINARY_MESSAGE_TAGS["extended_screen_zones"],
        1,
        2,
        3840,
        2160,
        4,
        1,
        0,
        0,
        0,
        0,
    ) + bytes(12)
    chunks = _screen_zone_chunks(encoded, 100, 30)
    stream = HypercolorEventStream(_TestClient())

    with pytest.raises(ValueError, match="start with chunk zero"):
        stream._decode_received_binary(chunks[1])
    assert isinstance(stream._decode_received_binary(chunks[0]), BinaryMessage)
    assert isinstance(stream._decode_received_binary(chunks[1]), ScreenZonesData)
    assert stream._screen_zones_reassembler.connection_bytes == 0

    duplicate_chunks = _screen_zone_chunks(encoded, 101, 30)
    duplicate_stream = HypercolorEventStream(_TestClient())
    duplicate_stream._decode_received_binary(duplicate_chunks[0])
    with pytest.raises(ValueError, match="duplicates"):
        duplicate_stream._decode_received_binary(duplicate_chunks[0])
    assert duplicate_stream._screen_zones_reassembler.reserved_bytes == 0


def test_screen_zone_chunks_reject_declared_publication_overflow() -> None:
    total = 536_936_449
    payload = (
        struct.pack(
            "<5BHQ4I2Q2I",
            ws_protocol.BINARY_MESSAGE_TAGS["preview_chunk"],
            1,
            3,
            ws_protocol.BINARY_MESSAGE_TAGS["screen_zones"],
            0,
            0,
            101,
            1,
            2,
            3840,
            2160,
            total,
            0,
            0,
            2,
        )
        + b"x"
    )

    with pytest.raises(ValueError, match="bounds"):
        HypercolorEventStream(_TestClient())._decode_received_binary(payload)


def test_malformed_newer_publication_does_not_advance_high_water() -> None:
    encoded = _extended_screen_zones_frame(4, 1, bytes(12))
    old_chunks = _screen_zone_chunks(encoded, 10, 30)
    stream = HypercolorEventStream(_TestClient())
    stream._decode_received_binary(old_chunks[0])
    total = websocket_module._PREVIEW_TRANSPORT_LIMITS["encoded"] + 1
    oversized_new = (
        struct.pack(
            "<5BHQ4I2Q2I",
            ws_protocol.BINARY_MESSAGE_TAGS["preview_chunk"],
            1,
            3,
            ws_protocol.BINARY_MESSAGE_TAGS["screen_zones"],
            0,
            0,
            (1 << 64) - 1,
            1,
            2,
            3840,
            2160,
            total,
            0,
            0,
            2,
        )
        + b"x"
    )

    with pytest.raises(ValueError, match="bounds"):
        stream._decode_received_binary(oversized_new)

    assert stream._screen_zones_reassembler.reserved_bytes == len(encoded)
    assert stream._screen_zones_reassembler.inbound_frame_bytes == 0
    assert stream._screen_zones_reassembler.decoded_bytes == 0
    assert stream._screen_zones_reassembler.connection_bytes == len(encoded)
    completed = stream._decode_received_binary(old_chunks[1])
    assert isinstance(completed, ScreenZonesData)
    assert stream._screen_zones_reassembler.connection_bytes == 0


def test_screen_zone_cancel_releases_reserved_publication(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    encoded = struct.pack(
        "<B10I",
        ws_protocol.BINARY_MESSAGE_TAGS["extended_screen_zones"],
        1,
        2,
        3840,
        2160,
        4,
        1,
        0,
        0,
        0,
        0,
    ) + bytes(12)
    chunks = _screen_zone_chunks(encoded, 102, 30)
    stream = HypercolorEventStream(_TestClient())
    stream._decode_received_binary(chunks[0])
    assert stream._screen_zones_reassembler.reserved_bytes == len(encoded)
    cancel_payload = _screen_zone_cancel(102)
    observed: list[tuple[int, int, int]] = []
    original = websocket_module._ScreenZonesChunkReassembler.cancel

    def observe_cancel(
        reassembler: websocket_module._ScreenZonesChunkReassembler,
        payload: bytes,
    ) -> None:
        observed.append(
            (
                reassembler.reserved_bytes,
                reassembler.inbound_frame_bytes,
                reassembler.connection_bytes,
            )
        )
        original(reassembler, payload)

    monkeypatch.setattr(
        websocket_module._ScreenZonesChunkReassembler,
        "cancel",
        observe_cancel,
    )

    cancelled = stream._decode_received_binary(cancel_payload)

    assert isinstance(cancelled, BinaryMessage)
    assert observed == [(len(encoded), len(cancel_payload), len(encoded) + len(cancel_payload))]
    assert stream._screen_zones_reassembler.reserved_bytes == 0
    assert stream._screen_zones_reassembler.inbound_frame_bytes == 0
    assert stream._screen_zones_reassembler.decoded_bytes == 0
    assert stream._screen_zones_reassembler.connection_bytes == 0
    with pytest.raises(ValueError, match="completed or cancelled"):
        stream._decode_received_binary(chunks[1])


@pytest.mark.parametrize(
    ("encoded", "stream_kind", "channel_tag", "identity", "expected_type"),
    [
        (
            struct.pack("<BIIHHB", 0x03, 7, 8, 2, 1, 0) + bytes(range(6)),
            0,
            0x03,
            b"",
            CanvasData,
        ),
        (
            struct.pack("<BII", 0x08, 7, 8)
            + uuid.UUID("11111111-1111-1111-1111-111111111111").bytes
            + uuid.UUID("22222222-2222-2222-2222-222222222222").bytes
            + struct.pack("<HHB", 2, 1, 0)
            + bytes(range(6)),
            1,
            0x08,
            uuid.UUID("11111111-1111-1111-1111-111111111111").bytes
            + uuid.UUID("22222222-2222-2222-2222-222222222222").bytes,
            ZonePreviewData,
        ),
        (
            struct.pack("<BBIIHHB", 0x0A, 4, 7, 8, 2, 1, 0) + b"main" + bytes(range(6)),
            2,
            0x0A,
            b"main",
            InteractivePreviewData,
        ),
        (
            struct.pack("<BBIIHHB", 0x07, 7, 7, 8, 2, 1, 0) + b"display" + bytes(range(6)),
            4,
            0x07,
            b"display",
            DisplayPreviewData,
        ),
    ],
)
def test_preview_transport_reassembles_every_non_screen_family(
    encoded: bytes,
    stream_kind: int,
    channel_tag: int,
    identity: bytes,
    expected_type: type[object],
) -> None:
    chunks = _preview_chunks(
        encoded,
        103,
        max(1, len(encoded) // 2),
        stream_kind=stream_kind,
        channel_tag=channel_tag,
        identity=identity,
        frame_number=7,
        timestamp_ms=8,
        width=2,
        height=1,
    )
    stream = HypercolorEventStream(_TestClient())

    for chunk in chunks[:-1]:
        assert isinstance(stream._decode_received_binary(chunk), BinaryMessage)
    completed = stream._decode_received_binary(chunks[-1])

    assert isinstance(completed, expected_type)
    assert cast(Any, completed).pixels == bytes(range(6))


@pytest.mark.parametrize(
    ("stream_kind", "channel_tag", "identity"),
    [(0, 0x03, b""), (1, 0x08, bytes(32)), (2, 0x0A, b"main"), (4, 0x07, b"display")],
)
def test_preview_cancel_retires_every_non_screen_family(
    stream_kind: int,
    channel_tag: int,
    identity: bytes,
) -> None:
    header_len = {0: 14, 1: 46}.get(stream_kind, 15 + len(identity))
    encoded = bytes(header_len + 6)
    chunks = _preview_chunks(
        encoded,
        104,
        len(encoded) // 2,
        stream_kind=stream_kind,
        channel_tag=channel_tag,
        identity=identity,
        frame_number=7,
        timestamp_ms=8,
        width=2,
        height=1,
    )
    stream = HypercolorEventStream(_TestClient())
    stream._decode_received_binary(chunks[0])

    stream._decode_received_binary(
        _preview_cancel(
            104,
            stream_kind=stream_kind,
            channel_tag=channel_tag,
            identity=identity,
        )
    )

    with pytest.raises(ValueError, match="completed or cancelled"):
        stream._decode_received_binary(chunks[1])


def test_screen_zone_chunk_idle_expiry_releases_reserved_bytes(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    now = 0.0
    monkeypatch.setattr("hypercolor.websocket.time.monotonic", lambda: now)
    encoded = struct.pack(
        "<B10I",
        ws_protocol.BINARY_MESSAGE_TAGS["extended_screen_zones"],
        1,
        2,
        3840,
        2160,
        4,
        1,
        0,
        0,
        0,
        0,
    ) + bytes(12)
    chunks = _screen_zone_chunks(encoded, 104, 30)
    stream = HypercolorEventStream(_TestClient())
    stream._decode_received_binary(chunks[0])
    assert stream._screen_zones_reassembler.reserved_bytes == len(encoded)

    now = 6.0

    assert stream._screen_zones_reassembler.reserved_bytes == 0
    assert stream._screen_zones_reassembler.inbound_frame_bytes == 0
    assert stream._screen_zones_reassembler.decoded_bytes == 0
    assert stream._screen_zones_reassembler.connection_bytes == 0
    with pytest.raises(ValueError, match="completed or cancelled"):
        stream._decode_received_binary(chunks[1])


@pytest.mark.asyncio
async def test_screen_zone_chunk_idle_timer_expires_without_more_messages(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setitem(websocket_module._PREVIEW_TRANSPORT_LIMITS, "idle_ms", 1)
    encoded = _extended_screen_zones_frame(4, 1, bytes(12))
    chunks = _screen_zone_chunks(encoded, 107, 30)
    stream = HypercolorEventStream(_TestClient())

    stream._decode_received_binary(chunks[0])
    await asyncio.sleep(0.02)

    assert stream._screen_zones_reassembler.reserved_bytes == 0


@pytest.mark.asyncio
async def test_disconnect_clears_screen_zone_partial_and_high_water() -> None:
    encoded = _extended_screen_zones_frame(4, 1, bytes(12))
    chunks = _screen_zone_chunks(encoded, 108, 30)
    stream = HypercolorEventStream(_TestClient())
    connection = _ClosableConnection()
    stream._connection = cast(Any, connection)
    stream._decode_received_binary(chunks[0])

    await stream.disconnect()

    assert connection.closed
    assert stream._screen_zones_reassembler.reserved_bytes == 0
    assert stream._screen_zones_reassembler.inbound_frame_bytes == 0
    assert stream._screen_zones_reassembler.decoded_bytes == 0
    assert stream._screen_zones_reassembler.connection_bytes == 0
    assert isinstance(stream._decode_received_binary(chunks[0]), BinaryMessage)


def test_screen_zone_completion_obeys_peak_connection_ledger(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    encoded = struct.pack(
        "<B10I",
        ws_protocol.BINARY_MESSAGE_TAGS["extended_screen_zones"],
        1,
        2,
        3840,
        2160,
        4,
        1,
        0,
        0,
        0,
        0,
    ) + bytes(12)
    chunks = _screen_zone_chunks(encoded, 105, 30)
    peak_bytes = len(encoded) + 12 + len(chunks[-1])
    monkeypatch.setitem(
        websocket_module._PREVIEW_TRANSPORT_LIMITS,
        "connection",
        peak_bytes - 1,
    )
    stream = HypercolorEventStream(_TestClient())
    stream._decode_received_binary(chunks[0])

    with pytest.raises(ValueError, match="connection byte ledger"):
        stream._decode_received_binary(chunks[1])
    assert stream._screen_zones_reassembler.reserved_bytes == 0
    assert stream._screen_zones_reassembler.inbound_frame_bytes == 0
    assert stream._screen_zones_reassembler.decoded_bytes == 0
    assert stream._screen_zones_reassembler.connection_bytes == 0

    monkeypatch.setitem(
        websocket_module._PREVIEW_TRANSPORT_LIMITS,
        "connection",
        peak_bytes,
    )
    admitted = HypercolorEventStream(_TestClient())
    admitted_chunks = _screen_zone_chunks(encoded, 106, 30)
    admitted._decode_received_binary(admitted_chunks[0])
    completed = admitted._decode_received_binary(admitted_chunks[1])
    assert isinstance(completed, ScreenZonesData)
    assert admitted._screen_zones_reassembler.reserved_bytes == 0
    assert admitted._screen_zones_reassembler.inbound_frame_bytes == 0
    assert admitted._screen_zones_reassembler.decoded_bytes == 0
    assert admitted._screen_zones_reassembler.connection_bytes == 0


def test_screen_zone_completion_retains_accounting_through_parse(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    rgb = bytes(12)
    encoded = _extended_screen_zones_frame(4, 1, rgb)
    chunks = _screen_zone_chunks(encoded, 111, 30)
    stream = HypercolorEventStream(_TestClient())
    original = HypercolorEventStream._parse_screen_zones
    observed: list[tuple[int, int, int, int]] = []

    def observe_accounting(payload: bytes | bytearray) -> ScreenZonesData:
        observed.append(
            (
                stream._screen_zones_reassembler.reserved_bytes,
                stream._screen_zones_reassembler.inbound_frame_bytes,
                stream._screen_zones_reassembler.decoded_bytes,
                stream._screen_zones_reassembler.connection_bytes,
            )
        )
        return original(payload)

    monkeypatch.setattr(
        HypercolorEventStream,
        "_parse_screen_zones",
        staticmethod(observe_accounting),
    )
    stream._decode_received_binary(chunks[0])

    completed = stream._decode_received_binary(chunks[1])

    assert isinstance(completed, ScreenZonesData)
    assert observed == [
        (len(encoded), len(chunks[1]), len(rgb), len(encoded) + len(chunks[1]) + len(rgb))
    ]
    assert stream._screen_zones_reassembler.reserved_bytes == 0
    assert stream._screen_zones_reassembler.inbound_frame_bytes == 0
    assert stream._screen_zones_reassembler.decoded_bytes == 0
    assert stream._screen_zones_reassembler.connection_bytes == 0


def test_screen_zone_parse_error_clears_all_transient_accounting() -> None:
    encoded = _extended_screen_zones_frame(5, 1, bytes(12))
    chunks = _screen_zone_chunks(encoded, 112, 30)
    stream = HypercolorEventStream(_TestClient())
    stream._decode_received_binary(chunks[0])

    with pytest.raises(ValueError, match="must be 15 bytes"):
        stream._decode_received_binary(chunks[1])

    assert stream._screen_zones_reassembler.reserved_bytes == 0
    assert stream._screen_zones_reassembler.inbound_frame_bytes == 0
    assert stream._screen_zones_reassembler.decoded_bytes == 0
    assert stream._screen_zones_reassembler.connection_bytes == 0


def test_screen_zone_completion_obeys_decoded_byte_ledger(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    rgb = bytes(12)
    encoded = _extended_screen_zones_frame(4, 1, rgb)
    chunks = _screen_zone_chunks(encoded, 109, 30)
    monkeypatch.setitem(
        websocket_module._PREVIEW_TRANSPORT_LIMITS,
        "decoded",
        len(rgb) - 1,
    )
    stream = HypercolorEventStream(_TestClient())
    with pytest.raises(ValueError, match="decoded byte ledger"):
        stream._decode_received_binary(chunks[0])
    assert stream._screen_zones_reassembler.reserved_bytes == 0
    assert stream._screen_zones_reassembler.inbound_frame_bytes == 0
    assert stream._screen_zones_reassembler.decoded_bytes == 0
    assert stream._screen_zones_reassembler.connection_bytes == 0

    monkeypatch.setitem(
        websocket_module._PREVIEW_TRANSPORT_LIMITS,
        "decoded",
        len(rgb),
    )
    admitted = HypercolorEventStream(_TestClient())
    admitted_chunks = _screen_zone_chunks(encoded, 110, 30)
    admitted._decode_received_binary(admitted_chunks[0])
    completed = admitted._decode_received_binary(admitted_chunks[1])
    assert isinstance(completed, ScreenZonesData)


def test_unknown_binary_tag_is_tolerated() -> None:
    payload = b"\x7f\x00\x01\x02"

    message = HypercolorEventStream._decode_binary(payload)

    assert isinstance(message, BinaryMessage)
    assert message.tag == 0x7F
    assert message.payload == payload


def test_client_messages_are_text_frames() -> None:
    encoded = _encode_text({"type": "subscribe", "topics": [{"topic": "frames"}]})

    assert isinstance(encoded, str)
