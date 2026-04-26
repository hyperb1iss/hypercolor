"""Library and preset models."""

from __future__ import annotations

from typing import Any

import msgspec


class Preset(msgspec.Struct, kw_only=True):
    """Named preset with serialized control values."""

    name: str
    is_default: bool = False
    controls: dict[str, Any] = msgspec.field(default_factory=dict)


class Playlist(msgspec.Struct, kw_only=True):
    """Placeholder playlist model for future API coverage."""

    id: str
    name: str
