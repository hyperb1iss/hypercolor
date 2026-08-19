"""Live scene-zone resource models."""

from __future__ import annotations

from typing import Any

import msgspec


class DisplayTarget(msgspec.Struct, kw_only=True):
    """Direct LCD target for a display-face zone."""

    device_id: str
    blend_mode: str = "alpha"
    opacity: float = 1.0


class SceneLayer(msgspec.Struct, kw_only=True):
    """One authored layer in a zone's bottom-to-top stack.

    ``source``, ``transform``, ``adjust``, and ``bindings`` are rich
    serde structures; they are carried as plain mappings so new fields
    never break decoding.
    """

    id: str
    source: dict[str, Any]
    name: str | None = None
    blend: str = "alpha"
    opacity: float = 1.0
    transform: dict[str, Any] = msgspec.field(default_factory=dict)
    adjust: dict[str, Any] = msgspec.field(default_factory=dict)
    bindings: list[dict[str, Any]] = msgspec.field(default_factory=list)
    enabled: bool = True


class ZoneMember(msgspec.Struct, kw_only=True):
    """One device segment assigned to a live zone."""

    id: str
    device_id: str
    name: str
    segment: str | None = None


class Zone(msgspec.Struct, kw_only=True):
    """One authored zone embedded in a live scene document."""

    id: str
    name: str
    role: str = "custom"
    enabled: bool = True
    brightness: float = 1.0
    color: str | None = None
    display_target: DisplayTarget | None = None
    members: list[ZoneMember] = msgspec.field(default_factory=list)
    layout: dict[str, Any] | None = None
    layers: list[SceneLayer] = msgspec.field(default_factory=list)

    @property
    def is_primary(self) -> bool:
        """Whether this zone is the scene's primary render group."""

        return self.role == "primary"

    @property
    def is_display(self) -> bool:
        """Whether this zone drives a display face."""

        return self.role == "display"
