"""Spatial layout models — the canvas-to-LED mapping vocabulary.

Mirrors the daemon's ``SpatialLayout``/``Output`` wire format. All
coordinates live in normalized ``[0.0, 1.0]`` canvas space.
"""

from __future__ import annotations

from typing import Any

import msgspec


class NormalizedPosition(msgspec.Struct, kw_only=True):
    """A position (or extent) in normalized ``[0.0, 1.0]`` canvas space."""

    x: float
    y: float


class LayoutOutput(msgspec.Struct, kw_only=True):
    """A device output: the spatial binding between a device zone and the canvas.

    ``topology``, ``sampling_mode``, ``edge_behavior``, and ``shape`` are
    serde-tagged enums on the wire; they are carried as plain mappings so
    new variants never break decoding.
    """

    id: str
    name: str
    device_id: str
    position: NormalizedPosition
    size: NormalizedPosition
    rotation: float
    topology: dict[str, Any]
    zone_name: str | None = None
    scale: float = 1.0
    display_order: int = 0
    orientation: str | None = None
    led_mapping: list[int] | None = None
    sampling_mode: dict[str, Any] | None = None
    edge_behavior: str | dict[str, Any] | None = None
    shape: dict[str, Any] | None = None
    shape_preset: str | None = None
    attachment: dict[str, Any] | None = None
    brightness: float | None = None

    @property
    def led_count(self) -> int:
        """Number of LEDs this output's topology produces."""

        topology = self.topology
        match topology.get("type"):
            case "strip" | "ring":
                count = int(topology.get("count", 0))
            case "matrix":
                count = int(topology.get("width", 0)) * int(topology.get("height", 0))
            case "concentric_rings":
                count = sum(int(ring.get("count", 0)) for ring in topology.get("rings", []))
            case "perimeter_loop":
                edges = ("top", "right", "bottom", "left")
                count = sum(int(topology.get(edge, 0)) for edge in edges)
            case "point":
                count = 1
            case "custom":
                count = len(topology.get("positions", []))
            case _:
                count = 0
        return count


class SpatialLayout(msgspec.Struct, kw_only=True):
    """Complete mapping from the 2D effect canvas to physical LED positions."""

    id: str
    name: str
    canvas_width: int
    canvas_height: int
    description: str | None = None
    zones: list[LayoutOutput] = msgspec.field(default_factory=list)
    default_sampling_mode: dict[str, Any] = msgspec.field(
        default_factory=lambda: {"type": "bilinear"}
    )
    default_edge_behavior: str | dict[str, Any] = "clamp"
    spaces: list[dict[str, Any]] | None = None
    version: int = 1
