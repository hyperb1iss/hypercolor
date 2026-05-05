"""Display face models."""

from __future__ import annotations

from typing import Any

import msgspec


class DisplaySummary(msgspec.Struct, kw_only=True):
    """Device that can render display face effects."""

    id: str
    name: str
    vendor: str
    family: str
    width: int
    height: int
    circular: bool


class DisplayFaceAssignment(msgspec.Struct, kw_only=True):
    """Active face assignment for a display device."""

    device_id: str
    scene_id: str
    effect: dict[str, Any]
    group: dict[str, Any]
