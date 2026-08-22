"""Layout models.

Layout detail endpoints return
:class:`hypercolor.models.spatial.SpatialLayout` directly.
"""

from __future__ import annotations

import msgspec


class LayoutSummary(msgspec.Struct, kw_only=True):
    """Layout data returned by list endpoints."""

    id: str
    name: str
    canvas_width: int
    canvas_height: int
    zone_count: int | None = None
    is_active: bool | None = None


__all__ = ["LayoutSummary"]
