"""WebSocket helpers for the Hypercolor daemon."""

from __future__ import annotations

import asyncio
import inspect
import struct
import time
import unicodedata
import uuid
from collections import defaultdict
from collections.abc import AsyncIterator, Callable, Mapping
from dataclasses import dataclass
from typing import Any, Never

import msgspec
from websockets import ConnectionClosed
from websockets.asyncio.client import ClientConnection, connect
from websockets.typing import Subprotocol

from .constants import WS_SUBPROTOCOL
from .ws_protocol import (
    BINARY_MESSAGE_TAGS,
    CANVAS_FORMAT_TAGS,
    PREVIEW_TOPIC_TAGS,
    WS_CAPABILITIES,
)

type JsonObject = dict[str, Any]
type EventHandler = Callable[[Any], Any]

_PREVIEW_TRANSPORT_PREFIX = "preview_transport_v1:"
_PREVIEW_CHUNK_HEADER_LEN = 55
_PREVIEW_CANCEL_HEADER_LEN = 14
_PREVIEW_TRANSPORT_CAPABILITY = next(
    capability
    for capability in WS_CAPABILITIES
    if capability.startswith(_PREVIEW_TRANSPORT_PREFIX)
)


def _preview_transport_limits() -> dict[str, int]:
    fields = _PREVIEW_TRANSPORT_CAPABILITY.removeprefix(_PREVIEW_TRANSPORT_PREFIX).split(",")
    return {name: int(value) for name, value in (field.split("=", 1) for field in fields)}


_PREVIEW_TRANSPORT_LIMITS = _preview_transport_limits()


@dataclass(slots=True)
class HelloMessage:
    """Initial hello payload sent by the daemon."""

    version: str
    state: JsonObject
    capabilities: list[str]
    subscriptions: list[str]


@dataclass(slots=True)
class EventMessage:
    """JSON event pushed by the daemon."""

    event: str
    timestamp: str
    data: JsonObject


@dataclass(slots=True)
class MetricsMessage:
    """Metrics payload pushed by the daemon."""

    timestamp: str
    data: JsonObject


@dataclass(slots=True)
class CommandResponse:
    """Response to a previously issued WebSocket command."""

    id: str
    status: int
    data: JsonObject | None = None
    error: JsonObject | None = None


@dataclass(slots=True)
class FrameZoneData:
    """LED payload for a single zone."""

    zone_id: str
    led_count: int
    rgb: bytes


@dataclass(slots=True)
class FrameData:
    """Binary LED frame payload."""

    frame_number: int
    timestamp_ms: int
    zones: list[FrameZoneData]


@dataclass(slots=True)
class SpectrumData:
    """Binary spectrum payload."""

    timestamp_ms: int
    bin_count: int
    level: float
    bass: float
    mid: float
    treble: float
    beat: bool
    beat_confidence: float
    bins: list[float]


@dataclass(slots=True)
class CanvasData:
    """Binary canvas payload."""

    frame_number: int
    timestamp_ms: int
    width: int
    height: int
    format: str
    pixels: bytes
    channel: str = "canvas"


@dataclass(slots=True)
class InteractivePreviewData:
    """Connection-scoped interactive preview frame."""

    preview_id: str
    frame_number: int
    timestamp_ms: int
    width: int
    height: int
    format: str
    pixels: bytes


@dataclass(slots=True)
class DisplayPreviewData:
    """One display's output frame, named by the device it came from."""

    device_id: str
    frame_number: int
    timestamp_ms: int
    width: int
    height: int
    format: str
    pixels: bytes


@dataclass(slots=True)
class ZonePreviewData:
    """Per-zone canvas preview payload."""

    scene_id: str
    zone_id: str
    frame_number: int
    timestamp_ms: int
    width: int
    height: int
    format: str
    pixels: bytes


@dataclass(slots=True)
class ScreenZonesData:
    """Ambilight zone grid payload — row-major RGB, ``cols * rows * 3`` bytes."""

    frame_number: int
    timestamp_ms: int
    source_width: int
    source_height: int
    grid_cols: int
    grid_rows: int
    letterbox: tuple[int, int, int, int]
    rgb: bytes


@dataclass(slots=True)
class BinaryMessage:
    """Unrecognized binary frame, carried whole for forward compatibility."""

    tag: int
    payload: bytes


@dataclass(slots=True)
class _PartialPreviewPublication:
    publication_id: int
    metadata: tuple[int, int, int, int, int, int, int]
    total_encoded_bytes: int
    chunk_count: int
    next_chunk_index: int
    encoded: bytearray
    last_activity: float
    completed: bool


@dataclass(frozen=True, slots=True)
class _ScreenZoneChunk:
    publication_id: int
    metadata: tuple[int, int, int, int, int, int, int]
    total_encoded_bytes: int
    chunk_offset: int
    chunk_index: int
    chunk_count: int
    payload: memoryview


def _parse_screen_zone_chunk(payload: bytes) -> _ScreenZoneChunk:
    if len(payload) > _PREVIEW_TRANSPORT_LIMITS["message"]:
        msg = "Preview chunk exceeds the negotiated message-byte limit"
        raise ValueError(msg)
    if len(payload) < _PREVIEW_CHUNK_HEADER_LEN:
        msg = "Preview chunk is shorter than its 55-byte header"
        raise ValueError(msg)
    (
        tag,
        schema,
        stream_kind,
        channel_tag,
        pixel_format,
        identity_len,
        publication_id,
        frame_number,
        timestamp_ms,
        width,
        height,
        total_encoded_bytes,
        chunk_offset,
        chunk_index,
        chunk_count,
    ) = struct.unpack_from("<5BHQ4I2Q2I", payload)
    if tag != BINARY_MESSAGE_TAGS["preview_chunk"] or schema != 1:
        msg = "Preview chunk has an unsupported tag or schema"
        raise ValueError(msg)
    payload_offset = _PREVIEW_CHUNK_HEADER_LEN + identity_len
    if len(payload) <= payload_offset:
        msg = "Preview chunk has a truncated identity or empty payload"
        raise ValueError(msg)
    payload_view = memoryview(payload)
    identity = payload_view[_PREVIEW_CHUNK_HEADER_LEN:payload_offset]
    if (
        stream_kind != 3
        or channel_tag != BINARY_MESSAGE_TAGS["screen_zones"]
        or pixel_format != 0
        or identity
    ):
        msg = "Preview chunk is not a screen-zone RGB publication"
        raise ValueError(msg)
    chunk_payload = payload_view[payload_offset:]
    end = chunk_offset + len(chunk_payload)
    if (
        total_encoded_bytes == 0
        or total_encoded_bytes > _PREVIEW_TRANSPORT_LIMITS["encoded"]
        or chunk_count == 0
        or chunk_count > _PREVIEW_TRANSPORT_LIMITS["chunks"]
        or chunk_count > total_encoded_bytes
        or chunk_index >= chunk_count
        or end > total_encoded_bytes
        or (chunk_index + 1 == chunk_count and end != total_encoded_bytes)
        or (chunk_index + 1 < chunk_count and end >= total_encoded_bytes)
    ):
        msg = "Preview chunk layout exceeds negotiated bounds"
        raise ValueError(msg)
    return _ScreenZoneChunk(
        publication_id=publication_id,
        metadata=(
            stream_kind,
            channel_tag,
            pixel_format,
            frame_number,
            timestamp_ms,
            width,
            height,
        ),
        total_encoded_bytes=total_encoded_bytes,
        chunk_offset=chunk_offset,
        chunk_index=chunk_index,
        chunk_count=chunk_count,
        payload=chunk_payload,
    )


class _ScreenZonesChunkReassembler:
    def __init__(self) -> None:
        self._partial: _PartialPreviewPublication | None = None
        self._high_water_publication_id: int | None = None
        self._reserved_bytes = 0
        self._inbound_frame_bytes = 0
        self._decoded_bytes = 0

    @property
    def reserved_bytes(self) -> int:
        self._expire_idle()
        return self._reserved_bytes

    @property
    def inbound_frame_bytes(self) -> int:
        return self._inbound_frame_bytes

    @property
    def decoded_bytes(self) -> int:
        return self._decoded_bytes

    @property
    def connection_bytes(self) -> int:
        return self._reserved_bytes + self._inbound_frame_bytes + self._decoded_bytes

    @property
    def has_partial(self) -> bool:
        return self._partial is not None

    def expire_partial(self) -> None:
        self._partial = None
        self._reserved_bytes = 0
        self._inbound_frame_bytes = 0
        self._decoded_bytes = 0

    def reset(self) -> None:
        self.expire_partial()
        self._high_water_publication_id = None

    def begin_inbound_frame(self, frame_bytes: int) -> None:
        self._inbound_frame_bytes = frame_bytes

    def finish_inbound_frame(self) -> None:
        self._inbound_frame_bytes = 0
        self._decoded_bytes = 0
        if self._partial is not None and self._partial.completed:
            self._partial = None
            self._reserved_bytes = 0

    def _expire_idle(self) -> None:
        partial = self._partial
        if partial is None:
            return
        idle_seconds = _PREVIEW_TRANSPORT_LIMITS["idle_ms"] / 1000
        if time.monotonic() - partial.last_activity >= idle_seconds:
            self.expire_partial()

    def _reject(self, message: str, publication_id: int | None = None) -> Never:
        if self._partial is not None and self._partial.publication_id == publication_id:
            self._partial = None
            self._reserved_bytes = 0
            self._decoded_bytes = 0
        raise ValueError(message)

    def push(self, payload: bytes) -> bytearray | None:
        self._expire_idle()
        publication_id = struct.unpack_from("<Q", payload, 7)[0] if len(payload) >= 15 else None
        starts_new = publication_id is not None and self._retire_superseded(publication_id)
        try:
            chunk = _parse_screen_zone_chunk(payload)
        except ValueError as exc:
            self._reject(str(exc), publication_id)
        partial = self._start_publication(chunk) if starts_new else self._publication_for(chunk)
        return self._append_chunk(partial, chunk)

    def _retire_superseded(self, publication_id: int) -> bool:
        if (
            self._high_water_publication_id is not None
            and publication_id <= self._high_water_publication_id
        ):
            return False
        self._partial = None
        self._reserved_bytes = 0
        self._decoded_bytes = 0
        self._high_water_publication_id = publication_id
        return True

    def _publication_for(self, chunk: _ScreenZoneChunk) -> _PartialPreviewPublication:
        partial = self._partial
        if (
            self._high_water_publication_id is not None
            and chunk.publication_id < self._high_water_publication_id
        ):
            msg = "Preview chunk belongs to a stale publication"
            raise ValueError(msg)
        if partial is None:
            msg = "Preview chunk duplicates a completed or cancelled publication"
            raise ValueError(msg)
        return partial

    def _start_publication(self, chunk: _ScreenZoneChunk) -> _PartialPreviewPublication:
        if chunk.chunk_index != 0 or chunk.chunk_offset != 0:
            msg = "Preview publication did not start with chunk zero"
            raise ValueError(msg)
        if (
            chunk.total_encoded_bytes + self._inbound_frame_bytes
            > _PREVIEW_TRANSPORT_LIMITS["connection"]
        ):
            msg = "Preview publication exceeds the connection byte ledger"
            raise ValueError(msg)
        partial = _PartialPreviewPublication(
            publication_id=chunk.publication_id,
            metadata=chunk.metadata,
            total_encoded_bytes=chunk.total_encoded_bytes,
            chunk_count=chunk.chunk_count,
            next_chunk_index=0,
            encoded=bytearray(),
            last_activity=time.monotonic(),
            completed=False,
        )
        self._partial = partial
        self._high_water_publication_id = chunk.publication_id
        self._reserved_bytes = chunk.total_encoded_bytes
        return partial

    def _append_chunk(
        self,
        partial: _PartialPreviewPublication,
        chunk: _ScreenZoneChunk,
    ) -> bytearray | None:
        if partial.metadata != chunk.metadata or (
            partial.total_encoded_bytes != chunk.total_encoded_bytes
            or partial.chunk_count != chunk.chunk_count
        ):
            self._reject(
                "Preview chunk metadata changed within a publication",
                chunk.publication_id,
            )
        if chunk.chunk_index < partial.next_chunk_index:
            self._reject("Preview chunk duplicates already received data", chunk.publication_id)
        if chunk.chunk_index != partial.next_chunk_index or chunk.chunk_offset != len(
            partial.encoded
        ):
            self._reject("Preview chunks are not contiguous", chunk.publication_id)
        if (
            partial.total_encoded_bytes + self._inbound_frame_bytes
            > _PREVIEW_TRANSPORT_LIMITS["connection"]
        ):
            self._reject(
                "Preview publication exceeds the connection byte ledger",
                chunk.publication_id,
            )
        try:
            partial.encoded.extend(chunk.payload)
        except MemoryError as exc:
            self._partial = None
            self._reserved_bytes = 0
            msg = "Preview publication buffer allocation failed"
            raise ValueError(msg) from exc
        partial.next_chunk_index += 1
        partial.last_activity = time.monotonic()
        if partial.next_chunk_index != partial.chunk_count:
            return None
        if len(partial.encoded) != partial.total_encoded_bytes:
            self._reject(
                "Preview chunks do not cover the declared publication length",
                chunk.publication_id,
            )
        return self._finish_publication(partial)

    def _finish_publication(self, partial: _PartialPreviewPublication) -> bytearray:
        completed = partial.encoded
        try:
            header_len = _validate_screen_zones_publication(completed, partial.metadata)
        except ValueError as exc:
            self._reject(str(exc), partial.publication_id)
        decoded_bytes = partial.total_encoded_bytes - header_len
        if decoded_bytes > _PREVIEW_TRANSPORT_LIMITS["decoded"]:
            self._reject(
                "Completed screen-zone publication exceeds the decoded byte ledger",
                partial.publication_id,
            )
        peak_bytes = partial.total_encoded_bytes + decoded_bytes + self._inbound_frame_bytes
        if peak_bytes > _PREVIEW_TRANSPORT_LIMITS["connection"]:
            self._reject(
                "Completed screen-zone publication exceeds the connection byte ledger",
                partial.publication_id,
            )
        self._decoded_bytes = decoded_bytes
        partial.completed = True
        return completed

    def cancel(self, payload: bytes) -> None:
        self._expire_idle()
        if len(payload) < _PREVIEW_CANCEL_HEADER_LEN:
            msg = "Preview cancellation is shorter than its 14-byte header"
            raise ValueError(msg)
        tag, schema, stream_kind, channel_tag, identity_len, publication_id = struct.unpack_from(
            "<4BHQ", payload
        )
        if tag != BINARY_MESSAGE_TAGS["preview_cancel"] or schema != 1:
            msg = "Preview cancellation has an unsupported tag or schema"
            raise ValueError(msg)
        if len(payload) != _PREVIEW_CANCEL_HEADER_LEN + identity_len:
            msg = "Preview cancellation identity length is invalid"
            raise ValueError(msg)
        identity = payload[_PREVIEW_CANCEL_HEADER_LEN:]
        if stream_kind != 3 or channel_tag != BINARY_MESSAGE_TAGS["screen_zones"] or identity:
            msg = "Preview cancellation is not for the screen-zone stream"
            raise ValueError(msg)
        if (
            self._high_water_publication_id is not None
            and publication_id < self._high_water_publication_id
        ):
            return
        self._high_water_publication_id = publication_id
        if self._partial is not None and self._partial.publication_id <= publication_id:
            self._partial = None
            self._reserved_bytes = 0
            self._decoded_bytes = 0


type WsMessage = (
    HelloMessage
    | EventMessage
    | MetricsMessage
    | CommandResponse
    | FrameData
    | SpectrumData
    | CanvasData
    | InteractivePreviewData
    | DisplayPreviewData
    | ZonePreviewData
    | ScreenZonesData
    | BinaryMessage
)

type _BinaryWsMessage = (
    FrameData
    | SpectrumData
    | CanvasData
    | InteractivePreviewData
    | DisplayPreviewData
    | ZonePreviewData
    | ScreenZonesData
    | BinaryMessage
)


def _is_screen_zone_preview_chunk(payload: bytes) -> bool:
    return (
        len(payload) >= _PREVIEW_CHUNK_HEADER_LEN
        and payload[2] == 3
        and payload[3] == BINARY_MESSAGE_TAGS["screen_zones"]
        and payload[5:7] == b"\x00\x00"
    )


def _is_screen_zone_preview_cancel(payload: bytes) -> bool:
    return (
        len(payload) >= _PREVIEW_CANCEL_HEADER_LEN
        and payload[2] == 3
        and payload[3] == BINARY_MESSAGE_TAGS["screen_zones"]
        and payload[4:6] == b"\x00\x00"
    )


def _screen_zones_header_len(payload: bytes | bytearray) -> int:
    tag = payload[0]
    if tag == BINARY_MESSAGE_TAGS["screen_zones"]:
        return 19
    if tag == BINARY_MESSAGE_TAGS["wide_screen_zones"]:
        return 23
    if tag == BINARY_MESSAGE_TAGS["extended_screen_zones"]:
        return 41
    msg = "Reassembled screen-zone publication has an unknown inner tag"
    raise ValueError(msg)


def _validate_screen_zones_publication(
    payload: bytes | bytearray,
    metadata: tuple[int, int, int, int, int, int, int],
) -> int:
    header_len = _screen_zones_header_len(payload)
    if len(payload) < header_len:
        msg = "Reassembled screen-zone publication has a truncated inner header"
        raise ValueError(msg)
    frame_number, timestamp_ms = struct.unpack_from("<II", payload, 1)
    if payload[0] == BINARY_MESSAGE_TAGS["screen_zones"]:
        source_width, source_height = struct.unpack_from("<HH", payload, 9)
    else:
        source_width, source_height = struct.unpack_from("<II", payload, 9)
    if (frame_number, timestamp_ms, source_width, source_height) != metadata[3:]:
        msg = "Reassembled screen-zone publication metadata changed"
        raise ValueError(msg)
    return header_len


class HypercolorEventStream:
    """WebSocket connection with channel subscriptions and event handlers."""

    def __init__(self, client: Any) -> None:
        self._url = client.ws_url
        self._api_key = client.api_key
        self._connection: ClientConnection | None = None
        self._handlers: dict[str, list[EventHandler]] = defaultdict(list)
        self._frame_handlers: list[EventHandler] = []
        self._spectrum_handlers: list[EventHandler] = []
        self._canvas_handlers: list[EventHandler] = []
        self._interactive_preview_handlers: list[EventHandler] = []
        self._display_preview_handlers: list[EventHandler] = []
        self._screen_zones_handlers: list[EventHandler] = []
        self._metrics_handlers: list[EventHandler] = []
        self._pending_responses: dict[str, asyncio.Future[CommandResponse]] = {}
        self._send_lock = asyncio.Lock()
        self._screen_zones_reassembler = _ScreenZonesChunkReassembler()
        self._screen_zones_expiry: asyncio.TimerHandle | None = None
        self.hello: HelloMessage | None = None

    async def __aenter__(self) -> HypercolorEventStream:
        await self.connect()
        return self

    async def __aexit__(self, *_exc_info: object) -> None:
        await self.disconnect()

    async def connect(self) -> HelloMessage:
        """Open the WebSocket connection and read the hello message."""
        self._reset_screen_zone_transport()
        headers = {}
        if self._api_key is not None:
            headers["Authorization"] = f"Bearer {self._api_key}"

        self._connection = await connect(
            self._url,
            additional_headers=headers or None,
            subprotocols=[Subprotocol(str(WS_SUBPROTOCOL))],
        )
        message = await self.receive()
        if not isinstance(message, HelloMessage):
            msg = "Expected hello message when establishing Hypercolor WebSocket connection"
            raise TypeError(msg)
        self.hello = message
        return message

    async def disconnect(self) -> None:
        """Close the WebSocket connection if it is open."""
        try:
            if self._connection is not None:
                await self._connection.close()
                self._connection = None
        finally:
            self._reset_screen_zone_transport()

    async def subscribe(
        self,
        *topics: str,
        key: str | None = None,
        config: Mapping[str, Any] | None = None,
    ) -> None:
        """Subscribe to one or more topics.

        A keyed topic (``display_preview``, ``interactive_preview``) takes
        its key here, so a call names one keyed subscription at a time.
        """
        await self.subscribe_many(
            [
                {
                    "topic": topic,
                    **({"key": key} if key is not None else {}),
                    **({"config": dict(config)} if config is not None else {}),
                }
                for topic in topics
            ]
        )

    async def subscribe_many(self, topics: list[Mapping[str, Any]]) -> None:
        """Subscribe to several topics, each with its own key and config."""
        payload: JsonObject = {
            "type": "subscribe",
            "topics": [dict(topic) for topic in topics],
            "preview_transport": _PREVIEW_TRANSPORT_CAPABILITY,
        }
        await self._send_json(payload)

    async def unsubscribe(self, *topics: str, key: str | None = None) -> None:
        """Unsubscribe from one or more topics."""
        await self._send_json(
            {
                "type": "unsubscribe",
                "topics": [
                    {"topic": topic, **({"key": key} if key is not None else {})}
                    for topic in topics
                ],
            }
        )

    async def open_interactive_preview(
        self,
        preview_id: str,
        *,
        fps: int,
        width: int,
        height: int,
        format: str = "jpeg",
        target: str = "active_scene",
    ) -> None:
        """Open or reconfigure one interactive preview.

        Opening is a keyed subscribe: the preview id is the key, and the
        daemon opens the render lane when the subscription is admitted.
        """
        await self.subscribe(
            "interactive_preview",
            key=preview_id,
            config={
                "target": target,
                "fps": fps,
                "width": width,
                "height": height,
                "format": format,
            },
        )

    async def close_interactive_preview(self, preview_id: str) -> None:
        """Close one interactive preview by retiring its subscription."""
        await self.unsubscribe("interactive_preview", key=preview_id)

    async def inject_preview_input(
        self,
        preview_id: str,
        events: list[Mapping[str, Any]],
    ) -> None:
        """Inject an ordered input batch into an active preview."""
        await self._send_json(
            {
                "type": "input_inject",
                "preview_id": preview_id,
                "events": [dict(event) for event in events],
            }
        )

    async def claim_interactive_preview(self, preview_id: str) -> None:
        """Claim an active preview as authoritative browser input."""
        await self._send_json(
            {
                "type": "interactive_preview_claim_authoritative",
                "preview_id": preview_id,
            }
        )

    async def release_interactive_preview(self, preview_id: str) -> None:
        """Release an active preview's authoritative browser-input claim."""
        await self._send_json(
            {
                "type": "interactive_preview_release_authoritative",
                "preview_id": preview_id,
            }
        )

    def on(self, event: str, handler: EventHandler) -> None:
        """Register a handler for a JSON event."""
        self._handlers[event].append(handler)

    def on_frames(self, handler: EventHandler) -> None:
        """Register a handler for LED frame messages."""
        self._frame_handlers.append(handler)

    def on_spectrum(self, handler: EventHandler) -> None:
        """Register a handler for spectrum messages."""
        self._spectrum_handlers.append(handler)

    def on_canvas(self, handler: EventHandler) -> None:
        """Register a handler for canvas preview messages."""
        self._canvas_handlers.append(handler)

    def on_interactive_preview(self, handler: EventHandler) -> None:
        """Register a handler for addressed interactive preview frames."""
        self._interactive_preview_handlers.append(handler)

    def on_display_preview(self, handler: EventHandler) -> None:
        """Register a handler for keyed display preview frames."""
        self._display_preview_handlers.append(handler)

    def on_screen_zones(self, handler: EventHandler) -> None:
        """Register a handler for screen zone-grid frames."""
        self._screen_zones_handlers.append(handler)

    def on_metrics(self, handler: EventHandler) -> None:
        """Register a handler for metrics messages."""
        self._metrics_handlers.append(handler)

    async def command(
        self,
        method: str,
        path: str,
        body: Mapping[str, Any] | None = None,
    ) -> CommandResponse:
        """Send a REST-like command over WebSocket and await its response."""
        connection = self._require_connection()
        correlation_id = f"cmd_{uuid.uuid4().hex[:12]}"
        future: asyncio.Future[CommandResponse] = asyncio.get_running_loop().create_future()
        self._pending_responses[correlation_id] = future
        payload = {
            "type": "command",
            "id": correlation_id,
            "method": method,
            "path": path,
            "body": dict(body) if body is not None else None,
        }
        async with self._send_lock:
            await connection.send(_encode_text(payload))

        while not future.done():
            await self.receive()
        return await future

    async def receive(self) -> WsMessage:
        """Receive and decode the next WebSocket message."""
        connection = self._require_connection()
        try:
            raw_message = await connection.recv()
        except ConnectionClosed as exc:
            self._connection = None
            self._reset_screen_zone_transport()
            msg = "Hypercolor WebSocket connection closed"
            raise RuntimeError(msg) from exc

        if isinstance(raw_message, bytes):
            message = self._decode_received_binary(raw_message)
            await self._dispatch_binary(message)
            return message

        message = self._decode_json(raw_message)
        await self._dispatch_json(message)
        return message

    async def __aiter__(self) -> AsyncIterator[WsMessage]:
        while True:
            yield await self.receive()

    async def _send_json(self, payload: JsonObject) -> None:
        connection = self._require_connection()
        async with self._send_lock:
            await connection.send(_encode_text(payload))

    def _require_connection(self) -> ClientConnection:
        if self._connection is None:
            msg = "Hypercolor WebSocket is not connected"
            raise RuntimeError(msg)
        return self._connection

    @staticmethod
    def _decode_json(raw_message: str) -> WsMessage:
        payload = msgspec.json.decode(raw_message.encode("utf-8"))
        if not isinstance(payload, dict):
            msg = "Unexpected non-object Hypercolor WebSocket message"
            raise TypeError(msg)

        message_type = payload.get("type")
        if message_type == "hello":
            return HelloMessage(
                version=str(payload["version"]),
                state=_expect_dict(payload.get("state")),
                capabilities=_expect_list_of_str(payload.get("capabilities")),
                subscriptions=_expect_list_of_str(payload.get("subscriptions")),
            )
        if message_type == "event":
            return EventMessage(
                event=str(payload["event"]),
                timestamp=str(payload["timestamp"]),
                data=_expect_dict(payload.get("data")),
            )
        if message_type == "metrics":
            return MetricsMessage(
                timestamp=str(payload["timestamp"]),
                data=_expect_dict(payload.get("data")),
            )
        if message_type == "response":
            return CommandResponse(
                id=str(payload["id"]),
                status=int(payload["status"]),
                data=_optional_dict(payload.get("data")),
                error=_optional_dict(payload.get("error")),
            )
        return EventMessage(
            event=str(message_type),
            timestamp=str(payload.get("timestamp", "")),
            data=_expect_dict(payload),
        )

    @staticmethod
    def _decode_binary(payload: bytes) -> _BinaryWsMessage:
        if not payload:
            msg = "Hypercolor binary message is empty"
            raise ValueError(msg)
        message_type = payload[0]
        if message_type == BINARY_MESSAGE_TAGS["led_frame"]:
            return HypercolorEventStream._parse_led_frame(payload)
        if message_type == BINARY_MESSAGE_TAGS["spectrum"]:
            return HypercolorEventStream._parse_spectrum(payload)
        if message_type in PREVIEW_TOPIC_TAGS:
            return HypercolorEventStream._parse_canvas(payload)
        return HypercolorEventStream._parse_special_binary(message_type, payload)

    def _decode_received_binary(self, payload: bytes) -> _BinaryWsMessage:
        if not payload:
            return self._decode_binary(payload)
        if payload[0] == BINARY_MESSAGE_TAGS["preview_chunk"]:
            return self._decode_preview_chunk(payload)
        if payload[0] == BINARY_MESSAGE_TAGS["preview_cancel"]:
            return self._decode_preview_cancel(payload)
        return self._decode_binary(payload)

    def _decode_preview_chunk(self, payload: bytes) -> _BinaryWsMessage:
        if not _is_screen_zone_preview_chunk(payload):
            return BinaryMessage(tag=payload[0], payload=payload)
        self._screen_zones_reassembler.begin_inbound_frame(len(payload))
        try:
            completed = self._screen_zones_reassembler.push(payload)
            if completed is None:
                return BinaryMessage(tag=payload[0], payload=payload)
            return self._parse_screen_zones(completed)
        finally:
            self._screen_zones_reassembler.finish_inbound_frame()
            self._refresh_screen_zone_expiry()

    def _decode_preview_cancel(self, payload: bytes) -> BinaryMessage:
        if not _is_screen_zone_preview_cancel(payload):
            return BinaryMessage(tag=payload[0], payload=payload)
        self._screen_zones_reassembler.begin_inbound_frame(len(payload))
        try:
            self._screen_zones_reassembler.cancel(payload)
        finally:
            self._screen_zones_reassembler.finish_inbound_frame()
            self._refresh_screen_zone_expiry()
        return BinaryMessage(tag=payload[0], payload=payload)

    def _refresh_screen_zone_expiry(self) -> None:
        if self._screen_zones_expiry is not None:
            self._screen_zones_expiry.cancel()
            self._screen_zones_expiry = None
        if not self._screen_zones_reassembler.has_partial:
            return
        try:
            loop = asyncio.get_running_loop()
        except RuntimeError:
            return
        idle_seconds = _PREVIEW_TRANSPORT_LIMITS["idle_ms"] / 1000
        self._screen_zones_expiry = loop.call_later(
            idle_seconds,
            self._expire_screen_zone_partial,
        )

    def _expire_screen_zone_partial(self) -> None:
        self._screen_zones_expiry = None
        self._screen_zones_reassembler.expire_partial()

    def _reset_screen_zone_transport(self) -> None:
        if self._screen_zones_expiry is not None:
            self._screen_zones_expiry.cancel()
            self._screen_zones_expiry = None
        self._screen_zones_reassembler.reset()

    @staticmethod
    def _parse_special_binary(message_type: int, payload: bytes) -> _BinaryWsMessage:
        if message_type == BINARY_MESSAGE_TAGS["zone_preview"]:
            return HypercolorEventStream._parse_zone_preview(payload)
        if message_type in (
            BINARY_MESSAGE_TAGS["screen_zones"],
            BINARY_MESSAGE_TAGS["wide_screen_zones"],
            BINARY_MESSAGE_TAGS["extended_screen_zones"],
        ):
            return HypercolorEventStream._parse_screen_zones(payload)
        if message_type == BINARY_MESSAGE_TAGS["interactive_preview"]:
            return HypercolorEventStream._parse_interactive_preview(payload)
        if message_type == BINARY_MESSAGE_TAGS["display_preview"]:
            return HypercolorEventStream._parse_display_preview(payload)
        return BinaryMessage(tag=message_type, payload=payload)

    async def _dispatch_json(self, message: WsMessage) -> None:
        if isinstance(message, CommandResponse):
            future = self._pending_responses.pop(message.id, None)
            if future is not None and not future.done():
                future.set_result(message)
            return
        if isinstance(message, EventMessage):
            for handler in self._handlers[message.event]:
                await _run_handler(handler, message)
            return
        if isinstance(message, MetricsMessage):
            for handler in self._metrics_handlers:
                await _run_handler(handler, message)

    async def _dispatch_binary(self, message: _BinaryWsMessage) -> None:
        if isinstance(message, FrameData):
            for handler in self._frame_handlers:
                await _run_handler(handler, message)
        elif isinstance(message, SpectrumData):
            for handler in self._spectrum_handlers:
                await _run_handler(handler, message)
        elif isinstance(message, CanvasData):
            for handler in self._canvas_handlers:
                await _run_handler(handler, message)
        elif isinstance(message, DisplayPreviewData):
            for handler in self._display_preview_handlers:
                await _run_handler(handler, message)
        elif isinstance(message, InteractivePreviewData):
            for handler in self._interactive_preview_handlers:
                await _run_handler(handler, message)
        elif isinstance(message, ScreenZonesData):
            for handler in self._screen_zones_handlers:
                await _run_handler(handler, message)

    @staticmethod
    def _parse_led_frame(payload: bytes) -> FrameData:
        frame_number, timestamp_ms = struct.unpack_from("<II", payload, 1)
        zone_count = payload[9]
        offset = 10
        zones: list[FrameZoneData] = []

        for _ in range(zone_count):
            zone_id_length = struct.unpack_from("<H", payload, offset)[0]
            offset += 2
            zone_id = payload[offset : offset + zone_id_length].decode("utf-8")
            offset += zone_id_length
            led_count = struct.unpack_from("<H", payload, offset)[0]
            offset += 2
            rgb_length = led_count * 3
            rgb = payload[offset : offset + rgb_length]
            offset += rgb_length
            zones.append(FrameZoneData(zone_id=zone_id, led_count=led_count, rgb=rgb))

        return FrameData(frame_number=frame_number, timestamp_ms=timestamp_ms, zones=zones)

    @staticmethod
    def _parse_spectrum(payload: bytes) -> SpectrumData:
        timestamp_ms = struct.unpack_from("<I", payload, 1)[0]
        bin_count = payload[5]
        level, bass, mid, treble = struct.unpack_from("<ffff", payload, 6)
        beat = bool(payload[22])
        beat_confidence = struct.unpack_from("<f", payload, 23)[0]
        bins_offset = 27
        bins = list(struct.unpack_from(f"<{bin_count}f", payload, bins_offset))
        return SpectrumData(
            timestamp_ms=timestamp_ms,
            bin_count=bin_count,
            level=level,
            bass=bass,
            mid=mid,
            treble=treble,
            beat=beat,
            beat_confidence=beat_confidence,
            bins=bins,
        )

    @staticmethod
    def _parse_canvas(payload: bytes) -> CanvasData:
        frame_number, timestamp_ms = struct.unpack_from("<II", payload, 1)
        width, height = struct.unpack_from("<HH", payload, 9)
        format_byte = payload[13]
        image_format = CANVAS_FORMAT_TAGS.get(format_byte)
        if image_format is None:
            msg = f"Unknown Hypercolor canvas format: {format_byte:#x}"
            raise RuntimeError(msg)
        pixels = payload[14:]
        return CanvasData(
            frame_number=frame_number,
            timestamp_ms=timestamp_ms,
            width=width,
            height=height,
            format=image_format,
            pixels=pixels,
            channel=PREVIEW_TOPIC_TAGS[payload[0]],
        )

    @staticmethod
    def _parse_zone_preview(payload: bytes) -> ZonePreviewData:
        frame_number, timestamp_ms = struct.unpack_from("<II", payload, 1)
        scene_id = uuid.UUID(bytes=payload[9:25])
        zone_id = uuid.UUID(bytes=payload[25:41])
        width, height = struct.unpack_from("<HH", payload, 41)
        return ZonePreviewData(
            scene_id=str(scene_id),
            zone_id=str(zone_id),
            frame_number=frame_number,
            timestamp_ms=timestamp_ms,
            width=width,
            height=height,
            format=CANVAS_FORMAT_TAGS.get(payload[45], "rgb"),
            pixels=payload[46:],
        )

    @staticmethod
    def _parse_display_preview(payload: bytes) -> DisplayPreviewData:
        """Decode a keyed display frame.

        Display and interactive previews share one identity-prefixed
        header layout, so this reuses that parse and renames the identity
        to the device it actually is.
        """
        frame = HypercolorEventStream._parse_identity_preview(
            payload, subject="Display preview device id"
        )
        return DisplayPreviewData(
            device_id=frame.preview_id,
            frame_number=frame.frame_number,
            timestamp_ms=frame.timestamp_ms,
            width=frame.width,
            height=frame.height,
            format=frame.format,
            pixels=frame.pixels,
        )

    @staticmethod
    def _parse_interactive_preview(payload: bytes) -> InteractivePreviewData:
        return HypercolorEventStream._parse_identity_preview(
            payload, subject="Interactive preview id"
        )

    @staticmethod
    def _parse_identity_preview(payload: bytes, *, subject: str) -> InteractivePreviewData:
        if len(payload) < 15:
            msg = f"{subject} frame is shorter than its prefix"
            raise ValueError(msg)
        preview_id_len = payload[1]
        if preview_id_len == 0:
            msg = f"{subject} cannot be empty"
            raise ValueError(msg)
        if preview_id_len > 128:
            msg = f"{subject} exceeds 128 bytes"
            raise ValueError(msg)
        payload_offset = 15 + preview_id_len
        if len(payload) < payload_offset:
            msg = f"{subject} frame has a truncated identity"
            raise ValueError(msg)
        frame_number, timestamp_ms = struct.unpack_from("<II", payload, 2)
        width, height = struct.unpack_from("<HH", payload, 10)
        format_byte = payload[14]
        image_format = CANVAS_FORMAT_TAGS.get(format_byte)
        if image_format is None:
            msg = f"Unknown {subject} preview format: {format_byte:#x}"
            raise ValueError(msg)
        preview_id = payload[15:payload_offset].decode("utf-8")
        if any(unicodedata.category(character) == "Cc" for character in preview_id):
            msg = f"{subject} contains a control character"
            raise ValueError(msg)
        image = payload[payload_offset:]
        bytes_per_pixel = {"rgb": 3, "rgba": 4}.get(image_format)
        if bytes_per_pixel is not None:
            expected = width * height * bytes_per_pixel
            if len(image) < expected:
                msg = (
                    f"{subject} payload is too short: "
                    f"expected {expected} bytes, got {len(image)}"
                )
                raise ValueError(msg)
            image = image[:expected]
        return InteractivePreviewData(
            preview_id=preview_id,
            frame_number=frame_number,
            timestamp_ms=timestamp_ms,
            width=width,
            height=height,
            format=image_format,
            pixels=image,
        )

    @staticmethod
    def _parse_screen_zones(payload: bytes | bytearray) -> ScreenZonesData:
        wide_source = payload[0] == BINARY_MESSAGE_TAGS["wide_screen_zones"]
        extended = payload[0] == BINARY_MESSAGE_TAGS["extended_screen_zones"]
        header_len = 41 if extended else 23 if wide_source else 19
        if len(payload) < header_len:
            msg = (
                f"Screen zones frame is shorter than its {header_len}-byte header: "
                f"{len(payload)} bytes"
            )
            raise ValueError(msg)
        if extended:
            (
                frame_number,
                timestamp_ms,
                source_width,
                source_height,
                grid_cols,
                grid_rows,
                letterbox_top,
                letterbox_bottom,
                letterbox_left,
                letterbox_right,
            ) = struct.unpack_from("<10I", payload, 1)
            payload_offset = header_len
        elif wide_source:
            frame_number, timestamp_ms = struct.unpack_from("<II", payload, 1)
            source_width, source_height = struct.unpack_from("<II", payload, 9)
            grid_cols = payload[17]
            grid_rows = payload[18]
            letterbox_top, letterbox_bottom, letterbox_left, letterbox_right = payload[19:23]
            payload_offset = header_len
        else:
            frame_number, timestamp_ms = struct.unpack_from("<II", payload, 1)
            source_width, source_height = struct.unpack_from("<HH", payload, 9)
            grid_cols = payload[13]
            grid_rows = payload[14]
            letterbox_top, letterbox_bottom, letterbox_left, letterbox_right = payload[15:19]
            payload_offset = header_len
        rgb = bytes(memoryview(payload)[payload_offset:])
        expected = grid_cols * grid_rows * 3
        if len(rgb) != expected:
            msg = f"Screen zones payload must be {expected} bytes, got {len(rgb)}"
            raise ValueError(msg)
        return ScreenZonesData(
            frame_number=frame_number,
            timestamp_ms=timestamp_ms,
            source_width=source_width,
            source_height=source_height,
            grid_cols=grid_cols,
            grid_rows=grid_rows,
            letterbox=(
                letterbox_top,
                letterbox_bottom,
                letterbox_left,
                letterbox_right,
            ),
            rgb=rgb,
        )


def _encode_text(payload: JsonObject) -> str:
    """Encode a client message as ``str`` so it ships as a WS text frame.

    The daemon only parses client messages from text frames; sending raw
    ``bytes`` would silently arrive as an ignored binary frame.
    """

    return msgspec.json.encode(payload).decode("utf-8")


async def _run_handler(handler: EventHandler, payload: Any) -> None:
    result = handler(payload)
    if inspect.isawaitable(result):
        await result


def _expect_dict(value: Any) -> JsonObject:
    if isinstance(value, dict):
        return value
    return {}


def _optional_dict(value: Any) -> JsonObject | None:
    return value if isinstance(value, dict) else None


def _expect_list_of_str(value: Any) -> list[str]:
    if not isinstance(value, list):
        return []
    return [str(item) for item in value]
