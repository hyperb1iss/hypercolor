"""Control-surface models."""

from __future__ import annotations

from typing import Any

import msgspec


class ControlSurface(msgspec.Struct, kw_only=True):
    """Dynamic device or driver control surface."""

    surface_id: str
    scope: dict[str, Any]
    schema_version: int
    revision: int
    groups: list[dict[str, Any]] = msgspec.field(default_factory=list)
    fields: list[dict[str, Any]] = msgspec.field(default_factory=list)
    actions: list[dict[str, Any]] = msgspec.field(default_factory=list)
    values: dict[str, Any] = msgspec.field(default_factory=dict)
    availability: dict[str, Any] = msgspec.field(default_factory=dict)
    action_availability: dict[str, Any] = msgspec.field(default_factory=dict)

    @property
    def id(self) -> str:
        """Alias for callers that expect resource-like identifiers."""
        return self.surface_id


class ControlApplyResult(msgspec.Struct, kw_only=True):
    """Response from applying control values."""

    surface_id: str
    previous_revision: int
    revision: int
    accepted: list[dict[str, Any]] = msgspec.field(default_factory=list)
    rejected: list[dict[str, Any]] = msgspec.field(default_factory=list)
    impacts: list[Any] = msgspec.field(default_factory=list)
    values: dict[str, Any] = msgspec.field(default_factory=dict)


class ControlActionResult(msgspec.Struct, kw_only=True):
    """Response from invoking a control action."""

    surface_id: str
    action_id: str
    status: str
    revision: int
    result: Any = None
