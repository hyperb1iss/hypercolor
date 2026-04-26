"""Layout models."""

from __future__ import annotations

import msgspec


class Point(msgspec.Struct, kw_only=True):
    """Normalized 2D point."""

    x: float
    y: float


class Size(msgspec.Struct, kw_only=True):
    """Normalized 2D size."""

    w: float
    h: float


class LayoutZone(msgspec.Struct, kw_only=True):
    """A mapped device zone inside a spatial layout."""

    device_id: str
    zone_id: str
    position: Point
    size: Size
    rotation: float
    topology: str
    led_count: int
    mirror: bool = False
    reverse: bool = False


class LayoutSummary(msgspec.Struct, kw_only=True):
    """Layout data returned by list endpoints."""

    id: str
    name: str
    canvas_width: int
    canvas_height: int
    zone_count: int | None = None
    is_active: bool | None = None


class Layout(LayoutSummary):
    """Full spatial layout with all zone positions."""

    zones: list[LayoutZone] = msgspec.field(default_factory=list)
