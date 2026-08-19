"""Scene models."""

from __future__ import annotations

from typing import Any

import msgspec

from .common import NamedRef
from .zone import ReplaceZoneRequest, Zone


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
    """A complete scene tree returned by live and stored-scene reads."""

    id: str
    name: str
    description: str | None = None
    kind: str = "named"
    is_default: bool = False
    unassigned_behavior: str | dict[str, Any] = "off"
    layout_id: str | None = None
    activation_brightness: float | None = None
    transition: dict[str, Any] = msgspec.field(
        default_factory=lambda: {
            "duration_ms": 1000,
            "easing": "Linear",
            "color_interpolation": "Oklab",
        }
    )
    priority: int = 50
    enabled: bool = True
    metadata: dict[str, str] = msgspec.field(default_factory=dict)
    mutation_mode: str = "live"
    revision: int = 0
    zones: list[Zone] = msgspec.field(default_factory=list)

    @property
    def primary_zone(self) -> Zone | None:
        """The zone with the primary role, if one exists."""

        return next((zone for zone in self.zones if zone.is_primary), None)

    def zone(self, zone_id: str) -> Zone | None:
        """Look up a zone by id."""

        return next((zone for zone in self.zones if zone.id == zone_id), None)


class ReplaceSceneRequest(msgspec.Struct, kw_only=True):
    """Whole-document body accepted by ``PUT /api/v1/scenes/{id}``."""

    name: str
    kind: str
    transition: dict[str, Any]
    priority: int
    enabled: bool
    id: str | None = None
    description: str | None = None
    unassigned_behavior: str | dict[str, Any] = "off"
    layout_id: str | None = None
    activation_brightness: float | None = None
    metadata: dict[str, str] = msgspec.field(default_factory=dict)
    mutation_mode: str = "live"
    zones: list[ReplaceZoneRequest] = msgspec.field(default_factory=list)

    @classmethod
    def from_document(cls, document: SceneDocument) -> ReplaceSceneRequest:
        """Strip response-only fields from a complete scene document."""

        return cls(
            id=document.id,
            name=document.name,
            description=document.description,
            kind=document.kind,
            unassigned_behavior=document.unassigned_behavior,
            layout_id=document.layout_id,
            activation_brightness=document.activation_brightness,
            transition=document.transition,
            priority=document.priority,
            enabled=document.enabled,
            metadata=document.metadata,
            mutation_mode=document.mutation_mode,
            zones=[ReplaceZoneRequest.from_zone(zone) for zone in document.zones],
        )


class ActivateSceneResult(msgspec.Struct, kw_only=True):
    """Response from manually triggering a scene."""

    scene: NamedRef
    activated: bool
