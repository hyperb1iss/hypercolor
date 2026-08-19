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
    description: str | None = None
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


class ReplaceSceneLayerRequest(msgspec.Struct, kw_only=True):
    """One complete layer in a stored-scene replacement."""

    source: dict[str, Any]
    id: str | None = None
    name: str | None = None
    blend: str = "alpha"
    opacity: float = 1.0
    transform: dict[str, Any] = msgspec.field(default_factory=dict)
    adjust: dict[str, Any] = msgspec.field(default_factory=dict)
    bindings: list[dict[str, Any]] = msgspec.field(default_factory=list)
    enabled: bool = True

    @classmethod
    def from_layer(cls, layer: SceneLayer) -> ReplaceSceneLayerRequest:
        """Build the replacement shape for a layer resource."""

        return cls(
            id=layer.id,
            name=layer.name,
            source=layer.source,
            blend=layer.blend,
            opacity=layer.opacity,
            transform=layer.transform,
            adjust=layer.adjust,
            bindings=layer.bindings,
            enabled=layer.enabled,
        )


class ReplaceZoneRequest(msgspec.Struct, kw_only=True):
    """One complete zone in a stored-scene replacement."""

    name: str
    id: str | None = None
    description: str | None = None
    role: str = "custom"
    enabled: bool = True
    brightness: float = 1.0
    color: str | None = None
    display_target: DisplayTarget | None = None
    members: list[ZoneMember] = msgspec.field(default_factory=list)
    layout: dict[str, Any] | None = None
    layers: list[ReplaceSceneLayerRequest] = msgspec.field(default_factory=list)

    @classmethod
    def from_zone(cls, zone: Zone) -> ReplaceZoneRequest:
        """Build the replacement shape for a zone resource."""

        return cls(
            id=zone.id,
            name=zone.name,
            description=zone.description,
            role=zone.role,
            enabled=zone.enabled,
            brightness=zone.brightness,
            color=zone.color,
            display_target=zone.display_target,
            members=zone.members,
            layout=zone.layout,
            layers=[ReplaceSceneLayerRequest.from_layer(layer) for layer in zone.layers],
        )
