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
    PREVIEW_TRANSPORT,
)

type JsonObject = dict[str, Any]
type EventHandler = Callable[[Any], Any]

_PREVIEW_CHUNK_HEADER_LEN = 55
_PREVIEW_CANCEL_HEADER_LEN = 14
_PREVIEW_TRANSPORT_LIMITS = {
    "decoded": int(PREVIEW_TRANSPORT["max_publication_decoded_bytes"]),
    "encoded": int(PREVIEW_TRANSPORT["max_publication_encoded_bytes"]),
    "connection": int(PREVIEW_TRANSPORT["max_connection_bytes"]),
    "reassembly": int(PREVIEW_TRANSPORT["max_reassembly_state_bytes"]),
    "tombstones": int(PREVIEW_TRANSPORT["max_tombstone_bytes"]),
    "sender": int(PREVIEW_TRANSPORT["max_sender_state_bytes"]),
    "cursors": int(PREVIEW_TRANSPORT["max_cursor_state_bytes"]),
    "idle_ms": int(PREVIEW_TRANSPORT["partial_idle_ms"]),
    "message": int(PREVIEW_TRANSPORT["max_message_bytes"]),
}


@dataclass(slots=True)
class ActiveSubscription:
    """One live subscription as the daemon reports it."""

    topic: str
    key: str | None = None
    config: JsonObject | None = None
    publication_id: int | None = None


@dataclass(slots=True)
class HelloMessage:
    """Initial hello payload sent by the daemon."""

    version: str
    state: JsonObject
    capabilities: list[str]
    subscriptions: list[ActiveSubscription]


@dataclass(slots=True)
class EventMessage:
    """JSON event pushed by the daemon."""

    event: str
    timestamp: str
    data: JsonObject


@dataclass(slots=True)
class SubscribedMessage:
    """Acknowledgment of the connection's complete live subscription set."""

    topics: list[ActiveSubscription]


@dataclass(slots=True)
class UnsubscribedMessage:
    """Acknowledgment of the connection's remaining live subscriptions."""

    topics: list[ActiveSubscription]


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
    stream: tuple[int, int, bytes]
    publication_id: int
    metadata: tuple[int, int, int, int, int, int, int]
    total_encoded_bytes: int
    chunk_count: int
    next_chunk_index: int
    encoded: bytearray
    last_activity: float
    completed: bool


@dataclass(frozen=True, slots=True)
class _PreviewChunk:
    stream: tuple[int, int, bytes]
    publication_id: int
    metadata: tuple[int, int, int, int, int, int, int]
    total_encoded_bytes: int
    chunk_offset: int
    chunk_index: int
    chunk_count: int
    payload: memoryview


@dataclass(slots=True)
class _PreviewStreamState:
    high_water_publication_id: int
    partial: _PartialPreviewPublication | None


@dataclass(frozen=True, slots=True)
class _CompletedPreviewPublication:
    stream: tuple[int, int, bytes]
    metadata: tuple[int, int, int, int, int, int, int]
    encoded: bytearray


def _validate_preview_identity(identity: bytes, subject: str) -> None:
    if not identity:
        msg = f"{subject} cannot be empty"
        raise ValueError(msg)
    if len(identity) > 128:
        msg = f"{subject} exceeds 128 bytes"
        raise ValueError(msg)
    try:
        decoded = identity.decode("utf-8")
    except UnicodeDecodeError as exc:
        msg = f"{subject} is not valid UTF-8"
        raise ValueError(msg) from exc
    if any(unicodedata.category(character) == "Cc" for character in decoded):
        msg = f"{subject} contains a control character"
        raise ValueError(msg)


def _parse_preview_stream(
    stream_kind: int, channel_tag: int, identity: bytes
) -> tuple[int, int, bytes]:
    if stream_kind == 0 and channel_tag in PREVIEW_TOPIC_TAGS and not identity:
        return stream_kind, channel_tag, identity
    if (
        stream_kind == 1
        and channel_tag == BINARY_MESSAGE_TAGS["zone_preview"]
        and len(identity) == 32
    ):
        return stream_kind, channel_tag, identity
    if stream_kind == 2 and channel_tag == BINARY_MESSAGE_TAGS["interactive_preview"]:
        _validate_preview_identity(identity, "Interactive preview id")
        return stream_kind, channel_tag, identity
    if stream_kind == 3 and channel_tag == BINARY_MESSAGE_TAGS["screen_zones"] and not identity:
        return stream_kind, channel_tag, identity
    if stream_kind == 4 and channel_tag == BINARY_MESSAGE_TAGS["display_preview"]:
        _validate_preview_identity(identity, "Display preview device id")
        return stream_kind, channel_tag, identity
    msg = "Preview transport stream identity is invalid"
    raise ValueError(msg)


def _preview_publication_header_len(
    stream: tuple[int, int, bytes], width: int, height: int
) -> int:
    stream_kind, _channel, identity = stream
    wide = width > 0xFFFF or height > 0xFFFF
    if stream_kind == 0:
        return 19 if wide else 14
    if stream_kind == 1:
        return 50 if wide else 46
    if stream_kind in (2, 4):
        return (19 if wide else 15) + len(identity)
    return 41 if wide else 19


def _validate_preview_chunk_layout(
    total_encoded_bytes: int,
    chunk_offset: int,
    chunk_index: int,
    chunk_count: int,
    chunk_payload_bytes: int,
) -> None:
    end = chunk_offset + chunk_payload_bytes
    if (
        total_encoded_bytes == 0
        or total_encoded_bytes > _PREVIEW_TRANSPORT_LIMITS["encoded"]
        or chunk_count == 0
        or chunk_count > total_encoded_bytes
        or chunk_index >= chunk_count
        or end > total_encoded_bytes
        or (chunk_index + 1 == chunk_count and end != total_encoded_bytes)
        or (chunk_index + 1 < chunk_count and end >= total_encoded_bytes)
    ):
        msg = "Preview chunk layout exceeds protocol bounds"
        raise ValueError(msg)


def _validate_preview_publication_admission(
    stream: tuple[int, int, bytes],
    pixel_format: int,
    width: int,
    height: int,
    total_encoded_bytes: int,
) -> None:
    if pixel_format not in CANVAS_FORMAT_TAGS:
        msg = "Preview chunk has an unknown pixel format"
        raise ValueError(msg)
    if width == 0 or height == 0:
        msg = "Preview chunk has invalid zero geometry"
        raise ValueError(msg)
    stream_kind = stream[0]
    if stream_kind == 3:
        if pixel_format != 0:
            msg = "Screen-zone preview chunks must use RGB"
            raise ValueError(msg)
        minimum_decoded = max(0, total_encoded_bytes - 41)
        if minimum_decoded > _PREVIEW_TRANSPORT_LIMITS["decoded"]:
            msg = "Preview publication exceeds the decoded byte ledger"
            raise ValueError(msg)
        return

    decoded_bytes = width * height * 4
    if decoded_bytes > _PREVIEW_TRANSPORT_LIMITS["decoded"]:
        msg = "Preview publication exceeds the decoded byte ledger"
        raise ValueError(msg)
    header_len = _preview_publication_header_len(stream, width, height)
    bytes_per_pixel = {0: 3, 1: 4}.get(pixel_format)
    if bytes_per_pixel is not None:
        expected = header_len + width * height * bytes_per_pixel
        if total_encoded_bytes != expected:
            msg = "Raw preview publication length does not match its geometry"
            raise ValueError(msg)
    elif total_encoded_bytes <= header_len:
        msg = "JPEG preview publication has an empty payload"
        raise ValueError(msg)


def _parse_preview_chunk(payload: bytes) -> _PreviewChunk:
    if len(payload) > _PREVIEW_TRANSPORT_LIMITS["message"]:
        msg = "Preview chunk exceeds the protocol message-byte limit"
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
    identity = bytes(payload_view[_PREVIEW_CHUNK_HEADER_LEN:payload_offset])
    stream = _parse_preview_stream(stream_kind, channel_tag, identity)
    chunk_payload = payload_view[payload_offset:]
    _validate_preview_chunk_layout(
        total_encoded_bytes,
        chunk_offset,
        chunk_index,
        chunk_count,
        len(chunk_payload),
    )
    _validate_preview_publication_admission(
        stream,
        pixel_format,
        width,
        height,
        total_encoded_bytes,
    )
    return _PreviewChunk(
        stream=stream,
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


def _parse_screen_zone_chunk(payload: bytes) -> _PreviewChunk:
    chunk = _parse_preview_chunk(payload)
    if chunk.stream[0] != 3:
        msg = "Preview chunk is not a screen-zone RGB publication"
        raise ValueError(msg)
    return chunk


class _PreviewChunkReassembler:
    def __init__(self) -> None:
        self._streams: dict[tuple[int, int, bytes], _PreviewStreamState] = {}
        self._reserved_bytes = 0
        self._inbound_frame_bytes = 0
        self._decoded_bytes = 0
        self._completed_stream: tuple[int, int, bytes] | None = None

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
        return any(state.partial is not None for state in self._streams.values())

    def expire_partial(self) -> None:
        for state in self._streams.values():
            state.partial = None
        self._reserved_bytes = 0
        self._inbound_frame_bytes = 0
        self._decoded_bytes = 0
        self._completed_stream = None

    def reset(self) -> None:
        self._streams.clear()
        self._reserved_bytes = 0
        self._inbound_frame_bytes = 0
        self._decoded_bytes = 0
        self._completed_stream = None

    def begin_inbound_frame(self, frame_bytes: int) -> None:
        self._inbound_frame_bytes = frame_bytes

    def finish_inbound_frame(self) -> None:
        self._inbound_frame_bytes = 0
        self._decoded_bytes = 0
        if self._completed_stream is not None:
            state = self._streams.get(self._completed_stream)
            if state is not None and state.partial is not None and state.partial.completed:
                self._reserved_bytes -= state.partial.total_encoded_bytes
                state.partial = None
            self._completed_stream = None

    def _expire_idle(self) -> None:
        idle_seconds = _PREVIEW_TRANSPORT_LIMITS["idle_ms"] / 1000
        now = time.monotonic()
        for state in self._streams.values():
            partial = state.partial
            if partial is not None and now - partial.last_activity >= idle_seconds:
                self._reserved_bytes -= partial.total_encoded_bytes
                state.partial = None

    def _reject(
        self,
        message: str,
        stream: tuple[int, int, bytes] | None = None,
        publication_id: int | None = None,
    ) -> Never:
        state = self._streams.get(stream) if stream is not None else None
        if (
            state is not None
            and state.partial is not None
            and state.partial.publication_id == publication_id
        ):
            self._reserved_bytes -= state.partial.total_encoded_bytes
            state.partial = None
            self._decoded_bytes = 0
        raise ValueError(message)

    def push(self, payload: bytes) -> _CompletedPreviewPublication | None:
        self._expire_idle()
        chunk = _parse_preview_chunk(payload)
        state = self._streams.get(chunk.stream)
        if state is not None and (
            chunk.publication_id < state.high_water_publication_id
            or (chunk.publication_id == state.high_water_publication_id and state.partial is None)
        ):
            msg = "Preview chunk duplicates a completed or cancelled publication"
            raise ValueError(msg)
        starts_new = state is None or chunk.publication_id > state.high_water_publication_id
        partial = self._start_publication(chunk) if starts_new else self._publication_for(chunk)
        return self._append_chunk(partial, chunk)

    def _publication_for(self, chunk: _PreviewChunk) -> _PartialPreviewPublication:
        state = self._streams.get(chunk.stream)
        partial = state.partial if state is not None else None
        if partial is None:
            msg = "Preview chunk duplicates a completed or cancelled publication"
            raise ValueError(msg)
        return partial

    def _start_publication(self, chunk: _PreviewChunk) -> _PartialPreviewPublication:
        if chunk.chunk_index != 0 or chunk.chunk_offset != 0:
            msg = "Preview publication did not start with chunk zero"
            raise ValueError(msg)
        prior_state = self._streams.get(chunk.stream)
        replaced_bytes = (
            prior_state.partial.total_encoded_bytes
            if prior_state is not None and prior_state.partial is not None
            else 0
        )
        if (
            self._reserved_bytes
            - replaced_bytes
            + chunk.total_encoded_bytes
            + self._inbound_frame_bytes
            > _PREVIEW_TRANSPORT_LIMITS["connection"]
        ):
            msg = "Preview publication exceeds the connection byte ledger"
            raise ValueError(msg)
        partial = _PartialPreviewPublication(
            stream=chunk.stream,
            publication_id=chunk.publication_id,
            metadata=chunk.metadata,
            total_encoded_bytes=chunk.total_encoded_bytes,
            chunk_count=chunk.chunk_count,
            next_chunk_index=0,
            encoded=bytearray(),
            last_activity=time.monotonic(),
            completed=False,
        )
        self._streams[chunk.stream] = _PreviewStreamState(
            high_water_publication_id=chunk.publication_id,
            partial=partial,
        )
        self._reserved_bytes = self._reserved_bytes - replaced_bytes + chunk.total_encoded_bytes
        return partial

    def _append_chunk(
        self,
        partial: _PartialPreviewPublication,
        chunk: _PreviewChunk,
    ) -> _CompletedPreviewPublication | None:
        if partial.metadata != chunk.metadata or (
            partial.total_encoded_bytes != chunk.total_encoded_bytes
            or partial.chunk_count != chunk.chunk_count
        ):
            self._reject(
                "Preview chunk metadata changed within a publication",
                chunk.stream,
                chunk.publication_id,
            )
        if chunk.chunk_index < partial.next_chunk_index:
            self._reject(
                "Preview chunk duplicates already received data",
                chunk.stream,
                chunk.publication_id,
            )
        if chunk.chunk_index != partial.next_chunk_index or chunk.chunk_offset != len(
            partial.encoded
        ):
            self._reject(
                "Preview chunks are not contiguous",
                chunk.stream,
                chunk.publication_id,
            )
        if (
            self._reserved_bytes + self._inbound_frame_bytes
            > _PREVIEW_TRANSPORT_LIMITS["connection"]
        ):
            self._reject(
                "Preview publication exceeds the connection byte ledger",
                chunk.stream,
                chunk.publication_id,
            )
        try:
            partial.encoded.extend(chunk.payload)
        except MemoryError as exc:
            state = self._streams[chunk.stream]
            state.partial = None
            self._reserved_bytes -= partial.total_encoded_bytes
            msg = "Preview publication buffer allocation failed"
            raise ValueError(msg) from exc
        partial.next_chunk_index += 1
        partial.last_activity = time.monotonic()
        if partial.next_chunk_index != partial.chunk_count:
            return None
        if len(partial.encoded) != partial.total_encoded_bytes:
            self._reject(
                "Preview chunks do not cover the declared publication length",
                chunk.stream,
                chunk.publication_id,
            )
        return self._finish_publication(partial)

    def _finish_publication(
        self, partial: _PartialPreviewPublication
    ) -> _CompletedPreviewPublication:
        stream_kind = partial.stream[0]
        if stream_kind == 3:
            header_len = _screen_zones_header_len(partial.encoded)
            decoded_bytes = partial.total_encoded_bytes - header_len
        else:
            decoded_bytes = partial.metadata[5] * partial.metadata[6] * 4
        if decoded_bytes > _PREVIEW_TRANSPORT_LIMITS["decoded"]:
            self._reject(
                "Completed preview publication exceeds the decoded byte ledger",
                partial.stream,
                partial.publication_id,
            )
        peak_bytes = self._reserved_bytes + decoded_bytes + self._inbound_frame_bytes
        if peak_bytes > _PREVIEW_TRANSPORT_LIMITS["connection"]:
            self._reject(
                "Completed preview publication exceeds the connection byte ledger",
                partial.stream,
                partial.publication_id,
            )
        self._decoded_bytes = decoded_bytes
        partial.completed = True
        self._completed_stream = partial.stream
        return _CompletedPreviewPublication(
            stream=partial.stream,
            metadata=partial.metadata,
            encoded=partial.encoded,
        )

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
        stream = _parse_preview_stream(stream_kind, channel_tag, identity)
        state = self._streams.get(stream)
        if state is None:
            self._streams[stream] = _PreviewStreamState(publication_id, None)
            return
        if publication_id < state.high_water_publication_id:
            return
        state.high_water_publication_id = publication_id
        if state.partial is not None and state.partial.publication_id <= publication_id:
            self._reserved_bytes -= state.partial.total_encoded_bytes
            state.partial = None
            self._decoded_bytes = 0


_ScreenZonesChunkReassembler = _PreviewChunkReassembler


type WsMessage = (
    HelloMessage
    | EventMessage
    | SubscribedMessage
    | UnsubscribedMessage
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


class HypercolorEventStream:
    """WebSocket connection with channel subscriptions and event handlers.

    The events channel carries live changes only and is never replayed.
    A stream that loses its socket misses every event during the gap, and
    the daemon does not resend them on reconnect, so refetch whatever you
    mirror each time the connection opens. Subscribe first and wait for
    the returned :class:`SubscribedMessage` before that REST refetch. The
    acknowledgment closes the gap between the REST snapshot and admission
    to the live event stream. Do the same whenever a ``resync_required``
    event arrives: the daemon sends it when a subscriber falls far enough
    behind that events were dropped on a socket that is still open.

    The handshake is deliberately thin for the same reason. It reports how
    the daemon is running, not what is rendering; read ``GET /api/v1/scene``
    for the live tree and follow this channel for changes.
    """

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
    ) -> SubscribedMessage:
        """Subscribe to one or more topics.

        A keyed topic (``display_preview``, ``interactive_preview``) takes
        its key here, so a call names one keyed subscription at a time.
        """
        return await self.subscribe_many(
            [
                {
                    "topic": topic,
                    **({"key": key} if key is not None else {}),
                    **({"config": dict(config)} if config is not None else {}),
                }
                for topic in topics
            ]
        )

    async def subscribe_many(self, topics: list[Mapping[str, Any]]) -> SubscribedMessage:
        """Subscribe atomically and wait until the daemon admits the set."""
        payload: JsonObject = {
            "type": "subscribe",
            "topics": [dict(topic) for topic in topics],
        }
        await self._send_json(payload)
        acknowledgment = await self._wait_for_subscription_ack(SubscribedMessage)
        assert isinstance(acknowledgment, SubscribedMessage)
        return acknowledgment

    async def unsubscribe(self, *topics: str, key: str | None = None) -> UnsubscribedMessage:
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
        acknowledgment = await self._wait_for_subscription_ack(UnsubscribedMessage)
        assert isinstance(acknowledgment, UnsubscribedMessage)
        return acknowledgment

    async def _wait_for_subscription_ack(
        self, expected: type[SubscribedMessage] | type[UnsubscribedMessage]
    ) -> SubscribedMessage | UnsubscribedMessage:
        while True:
            message = await self.receive()
            if isinstance(message, expected):
                return message
            if isinstance(message, EventMessage) and message.event == "error":
                detail = message.data.get("message", "subscription request was rejected")
                raise RuntimeError(str(detail))

    async def open_interactive_preview(
        self,
        preview_id: str,
        *,
        fps: int,
        width: int,
        height: int,
        format: str = "jpeg",
        target: str = "active_scene",
    ) -> SubscribedMessage:
        """Open or reconfigure one interactive preview.

        Opening is a keyed subscribe: the preview id is the key, and the
        daemon opens the render lane when the subscription is admitted.
        """
        return await self.subscribe(
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

    async def close_interactive_preview(self, preview_id: str) -> UnsubscribedMessage:
        """Close one interactive preview by retiring its subscription."""
        return await self.unsubscribe("interactive_preview", key=preview_id)

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
                subscriptions=_parse_subscriptions(payload.get("subscriptions")),
            )
        if message_type == "event":
            return EventMessage(
                event=str(payload["event"]),
                timestamp=str(payload["timestamp"]),
                data=_expect_dict(payload.get("data")),
            )
        subscription_message = _decode_subscription_message(payload, message_type)
        if subscription_message is not None:
            return subscription_message
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
        if (
            message_type in PREVIEW_TOPIC_TAGS
            or message_type == BINARY_MESSAGE_TAGS["wide_preview"]
        ):
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
        self._screen_zones_reassembler.begin_inbound_frame(len(payload))
        try:
            completed = self._screen_zones_reassembler.push(payload)
            if completed is None:
                return BinaryMessage(tag=payload[0], payload=payload)
            message = self._decode_binary(bytes(completed.encoded))
            self._validate_completed_preview(message, completed)
            return message
        finally:
            self._screen_zones_reassembler.finish_inbound_frame()
            self._refresh_screen_zone_expiry()

    def _decode_preview_cancel(self, payload: bytes) -> BinaryMessage:
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
        if message_type in (
            BINARY_MESSAGE_TAGS["zone_preview"],
            BINARY_MESSAGE_TAGS["wide_zone_preview"],
        ):
            return HypercolorEventStream._parse_zone_preview(
                payload,
                wide=message_type == BINARY_MESSAGE_TAGS["wide_zone_preview"],
            )
        if message_type in (
            BINARY_MESSAGE_TAGS["screen_zones"],
            BINARY_MESSAGE_TAGS["wide_screen_zones"],
            BINARY_MESSAGE_TAGS["extended_screen_zones"],
        ):
            return HypercolorEventStream._parse_screen_zones(payload)
        if message_type in (
            BINARY_MESSAGE_TAGS["interactive_preview"],
            BINARY_MESSAGE_TAGS["wide_interactive_preview"],
        ):
            return HypercolorEventStream._parse_interactive_preview(
                payload,
                wide=message_type == BINARY_MESSAGE_TAGS["wide_interactive_preview"],
            )
        if message_type in (
            BINARY_MESSAGE_TAGS["display_preview"],
            BINARY_MESSAGE_TAGS["wide_display_preview"],
        ):
            return HypercolorEventStream._parse_display_preview(
                payload,
                wide=message_type == BINARY_MESSAGE_TAGS["wide_display_preview"],
            )
        return BinaryMessage(tag=message_type, payload=payload)

    @staticmethod
    def _validate_completed_preview(
        message: _BinaryWsMessage,
        completed: _CompletedPreviewPublication,
    ) -> None:
        stream_kind, channel_tag, identity = completed.stream
        _, _, pixel_format, frame_number, timestamp_ms, width, height = completed.metadata
        image_format = CANVAS_FORMAT_TAGS[pixel_format]
        common = (
            getattr(message, "frame_number", None) == frame_number
            and getattr(message, "timestamp_ms", None) == timestamp_ms
        )
        if isinstance(message, ScreenZonesData):
            matches = (
                stream_kind == 3
                and common
                and message.source_width == width
                and message.source_height == height
                and image_format == "rgb"
            )
        else:
            matches = (
                common
                and getattr(message, "width", None) == width
                and getattr(message, "height", None) == height
                and getattr(message, "format", None) == image_format
            )
            if isinstance(message, CanvasData):
                matches = (
                    matches
                    and stream_kind == 0
                    and PREVIEW_TOPIC_TAGS.get(channel_tag) == message.channel
                    and not identity
                )
            elif isinstance(message, ZonePreviewData):
                matches = (
                    matches
                    and stream_kind == 1
                    and uuid.UUID(message.scene_id).bytes == identity[:16]
                    and uuid.UUID(message.zone_id).bytes == identity[16:]
                )
            elif isinstance(message, InteractivePreviewData):
                matches = matches and stream_kind == 2 and message.preview_id.encode() == identity
            elif isinstance(message, DisplayPreviewData):
                matches = matches and stream_kind == 4 and message.device_id.encode() == identity
            else:
                matches = False
        if not matches:
            msg = "Reassembled preview publication metadata changed"
            raise ValueError(msg)

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
        # The zone count is a u16: a u8 silently dropped every zone past
        # 255 on a large rig (spec 78 section 7.1).
        zone_count = struct.unpack_from("<H", payload, 9)[0]
        offset = 11
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
        wide = payload[0] == BINARY_MESSAGE_TAGS["wide_preview"]
        header_len = 19 if wide else 14
        if len(payload) < header_len:
            msg = f"Canvas frame is shorter than its {header_len}-byte header"
            raise ValueError(msg)
        metadata_offset = 1 if wide else 0
        channel_tag = payload[metadata_offset]
        if channel_tag not in PREVIEW_TOPIC_TAGS:
            msg = f"Unknown Hypercolor canvas channel: {channel_tag:#x}"
            raise ValueError(msg)
        frame_number, timestamp_ms = struct.unpack_from("<II", payload, 1 + metadata_offset)
        if wide:
            width, height = struct.unpack_from("<II", payload, 10)
        else:
            width, height = struct.unpack_from("<HH", payload, 9)
        format_byte = payload[header_len - 1]
        image_format = CANVAS_FORMAT_TAGS.get(format_byte)
        if image_format is None:
            msg = f"Unknown Hypercolor canvas format: {format_byte:#x}"
            raise RuntimeError(msg)
        pixels = HypercolorEventStream._validated_preview_payload(
            payload[header_len:],
            width,
            height,
            image_format,
            "Canvas",
        )
        return CanvasData(
            frame_number=frame_number,
            timestamp_ms=timestamp_ms,
            width=width,
            height=height,
            format=image_format,
            pixels=pixels,
            channel=PREVIEW_TOPIC_TAGS[channel_tag],
        )

    @staticmethod
    def _parse_zone_preview(payload: bytes, *, wide: bool = False) -> ZonePreviewData:
        header_len = 50 if wide else 46
        if len(payload) < header_len:
            msg = f"Zone preview frame is shorter than its {header_len}-byte header"
            raise ValueError(msg)
        frame_number, timestamp_ms = struct.unpack_from("<II", payload, 1)
        scene_id = uuid.UUID(bytes=payload[9:25])
        zone_id = uuid.UUID(bytes=payload[25:41])
        if wide:
            width, height = struct.unpack_from("<II", payload, 41)
        else:
            width, height = struct.unpack_from("<HH", payload, 41)
        format_byte = payload[header_len - 1]
        image_format = CANVAS_FORMAT_TAGS.get(format_byte)
        if image_format is None:
            msg = f"Unknown zone preview format: {format_byte:#x}"
            raise ValueError(msg)
        return ZonePreviewData(
            scene_id=str(scene_id),
            zone_id=str(zone_id),
            frame_number=frame_number,
            timestamp_ms=timestamp_ms,
            width=width,
            height=height,
            format=image_format,
            pixels=HypercolorEventStream._validated_preview_payload(
                payload[header_len:],
                width,
                height,
                image_format,
                "Zone preview",
            ),
        )

    @staticmethod
    def _parse_display_preview(payload: bytes, *, wide: bool = False) -> DisplayPreviewData:
        """Decode a keyed display frame.

        Display and interactive previews share one identity-prefixed
        header layout, so this reuses that parse and renames the identity
        to the device it actually is.
        """
        frame = HypercolorEventStream._parse_identity_preview(
            payload, subject="Display preview device id", wide=wide
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
    def _parse_interactive_preview(
        payload: bytes,
        *,
        wide: bool = False,
    ) -> InteractivePreviewData:
        return HypercolorEventStream._parse_identity_preview(
            payload, subject="Interactive preview id", wide=wide
        )

    @staticmethod
    def _parse_identity_preview(
        payload: bytes,
        *,
        subject: str,
        wide: bool = False,
    ) -> InteractivePreviewData:
        """Decode an identity-prefixed preview frame.

        Interactive and display previews share this layout. The wide form
        widens both dimensions to u32 and pushes the identity out by four
        bytes; nothing else moves.
        """
        prefix_len = 19 if wide else 15
        if len(payload) < prefix_len:
            msg = f"{subject} frame is shorter than its prefix"
            raise ValueError(msg)
        preview_id_len = payload[1]
        if preview_id_len == 0:
            msg = f"{subject} cannot be empty"
            raise ValueError(msg)
        if preview_id_len > 128:
            msg = f"{subject} exceeds 128 bytes"
            raise ValueError(msg)
        payload_offset = prefix_len + preview_id_len
        if len(payload) < payload_offset:
            msg = f"{subject} frame has a truncated identity"
            raise ValueError(msg)
        frame_number, timestamp_ms = struct.unpack_from("<II", payload, 2)
        if wide:
            width, height = struct.unpack_from("<II", payload, 10)
        else:
            width, height = struct.unpack_from("<HH", payload, 10)
        format_byte = payload[prefix_len - 1]
        image_format = CANVAS_FORMAT_TAGS.get(format_byte)
        if image_format is None:
            msg = f"Unknown {subject} preview format: {format_byte:#x}"
            raise ValueError(msg)
        preview_id = payload[prefix_len:payload_offset].decode("utf-8")
        if any(unicodedata.category(character) == "Cc" for character in preview_id):
            msg = f"{subject} contains a control character"
            raise ValueError(msg)
        image = HypercolorEventStream._validated_preview_payload(
            payload[payload_offset:],
            width,
            height,
            image_format,
            subject,
        )
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
    def _validated_preview_payload(
        image: bytes,
        width: int,
        height: int,
        image_format: str,
        subject: str,
    ) -> bytes:
        if width == 0 or height == 0:
            msg = f"{subject} has invalid zero geometry"
            raise ValueError(msg)
        bytes_per_pixel = {"rgb": 3, "rgba": 4}.get(image_format)
        if bytes_per_pixel is None:
            if not image:
                msg = f"{subject} JPEG payload cannot be empty"
                raise ValueError(msg)
            return image
        expected = width * height * bytes_per_pixel
        if len(image) < expected:
            msg = f"{subject} payload is too short: expected {expected} bytes, got {len(image)}"
            raise ValueError(msg)
        if len(image) > expected:
            msg = f"{subject} payload must be {expected} bytes, got {len(image)}"
            raise ValueError(msg)
        return image

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


def _decode_subscription_message(
    payload: JsonObject, message_type: Any
) -> SubscribedMessage | UnsubscribedMessage | None:
    if message_type == "subscribed":
        return SubscribedMessage(
            topics=_parse_subscriptions(payload.get("topics")),
        )
    if message_type == "unsubscribed":
        return UnsubscribedMessage(
            topics=_parse_subscriptions(payload.get("topics")),
        )
    return None


def _parse_subscriptions(value: Any) -> list[ActiveSubscription]:
    """Read the live subscription entries a hello or acknowledgment carries."""
    if not isinstance(value, list):
        return []
    entries: list[ActiveSubscription] = []
    for item in value:
        if not isinstance(item, dict) or not isinstance(item.get("topic"), str):
            continue
        key = item.get("key")
        publication_id = item.get("publication_id")
        entries.append(
            ActiveSubscription(
                topic=item["topic"],
                key=key if isinstance(key, str) else None,
                config=_optional_dict(item.get("config")),
                publication_id=publication_id if isinstance(publication_id, int) else None,
            )
        )
    return entries
