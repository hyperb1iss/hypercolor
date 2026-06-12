"""Zone (render group) models — independent rendering pipelines within a scene.

A zone owns an effect (or layer stack), a spatial layout claiming device
outputs, and per-zone brightness. Zone structure mutations are guarded by
the scene's ``groups_revision`` via ``If-Match`` preconditions.
"""

from __future__ import annotations

from typing import Any

import msgspec

from .spatial import SpatialLayout


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


class Zone(msgspec.Struct, kw_only=True):
    """An independent rendering pipeline within a scene."""

    id: str
    name: str
    layout: SpatialLayout
    description: str | None = None
    effect_id: str | None = None
    controls: dict[str, Any] = msgspec.field(default_factory=dict)
    control_bindings: dict[str, Any] = msgspec.field(default_factory=dict)
    preset_id: str | None = None
    layers: list[SceneLayer] = msgspec.field(default_factory=list)
    brightness: float = 1.0
    enabled: bool = True
    color: str | None = None
    display_target: DisplayTarget | None = None
    role: str = "custom"
    controls_version: int = 0
    layers_version: int = 0

    @property
    def is_primary(self) -> bool:
        """Whether this zone is the scene's primary render group."""

        return self.role == "primary"

    @property
    def is_display(self) -> bool:
        """Whether this zone drives a display face."""

        return self.role == "display"


class ZoneListResult(msgspec.Struct, kw_only=True):
    """Zone set of a scene plus the revision guarding its structure."""

    items: list[Zone]
    groups_revision: int


class ZoneResult(msgspec.Struct, kw_only=True):
    """One zone after a create/get/update."""

    zone: Zone
    groups_revision: int


class ZoneDeleteResult(msgspec.Struct, kw_only=True):
    """Result of deleting a zone."""

    zone_id: str
    deleted: bool
    groups_revision: int


class UnassignedBehaviorResult(msgspec.Struct, kw_only=True):
    """Scene policy for device outputs claimed by no zone."""

    unassigned_behavior: str | dict[str, Any]
    groups_revision: int
