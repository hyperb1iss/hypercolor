"""Library and preset models."""

from __future__ import annotations

from typing import Any

import msgspec

from .common import NamedRef


class Favorite(msgspec.Struct, kw_only=True):
    """Favorite effect entry."""

    effect_id: str
    effect_name: str
    added_at_ms: int


class Preset(msgspec.Struct, kw_only=True):
    """Saved effect preset."""

    id: str
    name: str
    description: str | None = None
    effect_id: str
    controls: dict[str, Any] = msgspec.field(default_factory=dict)
    tags: list[str] = msgspec.field(default_factory=list)
    created_at_ms: int | None = None
    updated_at_ms: int | None = None


class PresetApplyResult(msgspec.Struct, kw_only=True):
    """Response from applying a saved preset."""

    preset: NamedRef
    effect: NamedRef
    applied_controls: dict[str, Any] = msgspec.field(default_factory=dict)
    rejected_controls: list[str] = msgspec.field(default_factory=list)
    warnings: list[str] = msgspec.field(default_factory=list)


class PlaylistItem(msgspec.Struct, kw_only=True):
    """One entry in a saved playlist."""

    id: str
    target: dict[str, Any]
    duration_ms: int | None = None
    transition_ms: int | None = None


class Playlist(msgspec.Struct, kw_only=True):
    """Saved effect playlist."""

    id: str
    name: str
    description: str | None = None
    items: list[PlaylistItem] = msgspec.field(default_factory=list)
    loop_enabled: bool = True
    created_at_ms: int | None = None
    updated_at_ms: int | None = None
