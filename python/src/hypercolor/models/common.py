"""Common model types shared across resource modules."""

from __future__ import annotations

from typing import Any

import msgspec

type JsonScalar = None | bool | int | float | str
type JsonValue = JsonScalar | list["JsonValue"] | dict[str, "JsonValue"]
type JsonObject = dict[str, JsonValue]


class Meta(msgspec.Struct, kw_only=True):
    """Standard response metadata."""

    api_version: str
    request_id: str
    timestamp: str


class ApiErrorBody(msgspec.Struct, kw_only=True):
    """Machine-readable daemon error payload."""

    code: str
    message: str
    details: dict[str, Any] | None = None


class ErrorEnvelope(msgspec.Struct, kw_only=True):
    """Daemon error response envelope."""

    error: ApiErrorBody
    meta: Meta


class Pagination(msgspec.Struct, kw_only=True):
    """Pagination metadata for list endpoints."""

    offset: int
    limit: int
    total: int
    has_more: bool


class NamedRef(msgspec.Struct, kw_only=True):
    """Compact object references used throughout the API."""

    id: str
    name: str


class MutationResult(msgspec.Struct, kw_only=True):
    """Simple resource mutation status."""

    id: str | None = None
    removed: bool | None = None
    deleted: bool | None = None
    enabled: bool | None = None
    paused: bool | None = None
    applied: bool | None = None
    activated: bool | None = None
    reset: bool | None = None
    stopped: bool | None = None


class DiscoverResult(msgspec.Struct, kw_only=True):
    """Response from a device discovery request."""

    scan_id: str
    status: str
    backends: list[str] = msgspec.field(default_factory=list)
    timeout_ms: int | None = None
    result: dict[str, Any] | None = None


class IdentifyResult(msgspec.Struct, kw_only=True):
    """Response from a device identify request."""

    device_id: str
    identifying: bool
    duration_ms: int


class BrightnessUpdate(msgspec.Struct, kw_only=True):
    """Response from a brightness mutation."""

    brightness: int


class ConfigMutationResult(msgspec.Struct, kw_only=True):
    """Generic response from config mutation endpoints."""

    key: str | None = None
    value: Any = None
    live: bool | None = None
    reset: bool | None = None
    path: str | None = None


class TransitionSpec(msgspec.Struct, kw_only=True):
    """Transition description for effect/profile/scene application."""

    type: str
    duration_ms: int = 300
