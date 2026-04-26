"""Scene models."""

from __future__ import annotations

import msgspec

from .common import NamedRef


class Scene(msgspec.Struct, kw_only=True):
    """Scene summary returned by the daemon."""

    id: str
    name: str
    description: str | None = None
    enabled: bool = True
    priority: int = 0


class ActivateSceneResult(msgspec.Struct, kw_only=True):
    """Response from manually triggering a scene."""

    scene: NamedRef
    activated: bool
