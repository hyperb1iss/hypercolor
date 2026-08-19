"""Scene models."""

from __future__ import annotations

from typing import Any

import msgspec

from .common import NamedRef
from .zone import Zone


class Scene(msgspec.Struct, kw_only=True):
    """Scene summary returned by the daemon."""

    id: str
    name: str
    description: str | None = None
    enabled: bool = True
    priority: int = 0
    mutation_mode: str = "live"

    @property
    def snapshot_locked(self) -> bool:
        """Whether live runtime actions are blocked from rewriting this scene."""

        return self.mutation_mode == "snapshot"


class SceneDocument(msgspec.Struct, kw_only=True):
    """The complete live scene tree returned by ``GET /api/v1/scene``."""

    id: str
    name: str
    kind: str = "named"
    is_default: bool = False
    unassigned_behavior: str | dict[str, Any] = "off"
    layout_id: str | None = None
    revision: int = 0
    zones: list[Zone] = msgspec.field(default_factory=list)

    @property
    def primary_zone(self) -> Zone | None:
        """The zone with the primary role, if one exists."""

        return next((zone for zone in self.zones if zone.is_primary), None)

    def zone(self, zone_id: str) -> Zone | None:
        """Look up a zone by id."""

        return next((zone for zone in self.zones if zone.id == zone_id), None)


class ActivateSceneResult(msgspec.Struct, kw_only=True):
    """Response from manually triggering a scene."""

    scene: NamedRef
    activated: bool
