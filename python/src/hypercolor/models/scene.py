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


class ActiveScene(msgspec.Struct, kw_only=True):
    """The active scene with its full render-group (zone) set.

    ``groups_revision`` is the monotonic structure counter carried as the
    ``If-Match`` precondition for every zone mutation.
    """

    id: str
    name: str
    description: str | None = None
    enabled: bool = True
    priority: int = 0
    kind: str = "named"
    mutation_mode: str = "live"
    groups: list[Zone] = msgspec.field(default_factory=list)
    groups_revision: int = 0
    unassigned_behavior: str | dict[str, Any] = "off"

    @property
    def primary_zone(self) -> Zone | None:
        """The zone with the primary role, if one exists."""

        return next((zone for zone in self.groups if zone.is_primary), None)

    def zone(self, zone_id: str) -> Zone | None:
        """Look up a zone by id."""

        return next((zone for zone in self.groups if zone.id == zone_id), None)


class ActivateSceneResult(msgspec.Struct, kw_only=True):
    """Response from manually triggering a scene."""

    scene: NamedRef
    activated: bool


class DeactivateSceneResult(msgspec.Struct, kw_only=True):
    """Response from returning to the synthesized default scene."""

    deactivated: bool
    previous_scene: Scene | None = None
    scene: Scene | None = None
