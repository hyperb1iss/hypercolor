"""Layout models.

Layout detail endpoints (``GET /layouts/{id}``, ``GET /layouts/active``)
return the raw spatial layout; see :mod:`hypercolor.models.spatial`.
"""

from __future__ import annotations

import msgspec

from .spatial import LayoutOutput, NormalizedPosition, SpatialLayout

Layout = SpatialLayout
"""Full spatial layout — alias for :class:`SpatialLayout`."""


class LayoutSummary(msgspec.Struct, kw_only=True):
    """Layout data returned by list endpoints."""

    id: str
    name: str
    canvas_width: int
    canvas_height: int
    zone_count: int | None = None
    is_active: bool | None = None


__all__ = ["Layout", "LayoutOutput", "LayoutSummary", "NormalizedPosition", "SpatialLayout"]
