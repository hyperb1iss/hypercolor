"""WebSocket helpers for the Hypercolor daemon."""

from __future__ import annotations

import asyncio
import inspect
import struct
import uuid
from collections import defaultdict
from collections.abc import AsyncIterator, Callable, Mapping
from dataclasses import dataclass
from typing import Any

import msgspec
from websockets import ConnectionClosed
from websockets.asyncio.client import ClientConnection, connect
from websockets.typing import Subprotocol

from .constants import WS_SUBPROTOCOL

type JsonObject = dict[str, Any]
type EventHandler = Callable[[Any], Any]


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


type WsMessage = (
    HelloMessage
    | EventMessage
    | MetricsMessage
    | CommandResponse
    | FrameData
    | SpectrumData
    | CanvasData
)


class HypercolorEventStream:
    """WebSocket connection with channel subscriptions and event handlers."""

    def __init__(self, client: Any) -> None:
        self._url = client.ws_url
        self._api_key = client.api_key
        self._connection: ClientConnection | None = None
        self._handlers: dict[str, list[EventHandler]] = defaultdict(list)
        self._frame_handlers: list[EventHandler] = []
        self._spectrum_handlers: list[EventHandler] = []
        self._metrics_handlers: list[EventHandler] = []
        self._pending_responses: dict[str, asyncio.Future[CommandResponse]] = {}
        self._send_lock = asyncio.Lock()
        self.hello: HelloMessage | None = None

    async def __aenter__(self) -> HypercolorEventStream:
        await self.connect()
        return self

    async def __aexit__(self, *_exc_info: object) -> None:
        await self.disconnect()

    async def connect(self) -> HelloMessage:
        """Open the WebSocket connection and read the hello message."""
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
        if self._connection is not None:
            await self._connection.close()
            self._connection = None

    async def subscribe(
        self,
        *channels: str,
        config: Mapping[str, Any] | None = None,
    ) -> None:
        """Subscribe to one or more channels."""
        payload: JsonObject = {"type": "subscribe", "channels": list(channels)}
        if config is not None:
            payload["config"] = dict(config)
        await self._send_json(payload)

    async def unsubscribe(self, *channels: str) -> None:
        """Unsubscribe from one or more channels."""
        await self._send_json({"type": "unsubscribe", "channels": list(channels)})

    def on(self, event: str, handler: EventHandler) -> None:
        """Register a handler for a JSON event."""
        self._handlers[event].append(handler)

    def on_frames(self, handler: EventHandler) -> None:
        """Register a handler for LED frame messages."""
        self._frame_handlers.append(handler)

    def on_spectrum(self, handler: EventHandler) -> None:
        """Register a handler for spectrum messages."""
        self._spectrum_handlers.append(handler)

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
            await connection.send(msgspec.json.encode(payload))

        while not future.done():
            await self.receive()
        return await future

    async def receive(self) -> WsMessage:
        """Receive and decode the next WebSocket message."""
        connection = self._require_connection()
        try:
            raw_message = await connection.recv()
        except ConnectionClosed as exc:
            msg = "Hypercolor WebSocket connection closed"
            raise RuntimeError(msg) from exc

        if isinstance(raw_message, bytes):
            message = self._decode_binary(raw_message)
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
            await connection.send(msgspec.json.encode(payload))

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
    def _decode_binary(payload: bytes) -> FrameData | SpectrumData | CanvasData:
        message_type = payload[0]
        if message_type == 0x01:
            return HypercolorEventStream._parse_led_frame(payload)
        if message_type == 0x02:
            return HypercolorEventStream._parse_spectrum(payload)
        if message_type == 0x03:
            return HypercolorEventStream._parse_canvas(payload)
        msg = f"Unknown Hypercolor binary message type: {message_type:#x}"
        raise RuntimeError(msg)

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

    async def _dispatch_binary(self, message: FrameData | SpectrumData | CanvasData) -> None:
        if isinstance(message, FrameData):
            for handler in self._frame_handlers:
                await _run_handler(handler, message)
        elif isinstance(message, SpectrumData):
            for handler in self._spectrum_handlers:
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
        image_format = "rgba" if format_byte == 1 else "rgb"
        pixels = payload[14:]
        return CanvasData(
            frame_number=frame_number,
            timestamp_ms=timestamp_ms,
            width=width,
            height=height,
            format=image_format,
            pixels=pixels,
        )


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
