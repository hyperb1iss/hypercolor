"""Profile models."""

from __future__ import annotations

from typing import Any

import msgspec

from .common import TransitionSpec


class ProfileSummary(msgspec.Struct, kw_only=True):
    """Profile list entry."""

    id: str
    name: str
    description: str | None = None
    brightness: int | None = None
    effect_id: str | None = None
    effect_name: str | None = None
    active_preset_id: str | None = None
    controls: dict[str, Any] = msgspec.field(default_factory=dict)
    layout_id: str | None = None


class Profile(ProfileSummary):
    """Full saved profile."""


class ApplyProfileResult(msgspec.Struct, kw_only=True):
    """Response from applying a profile."""

    profile: Profile
    applied: bool
    transition: TransitionSpec | None = None
