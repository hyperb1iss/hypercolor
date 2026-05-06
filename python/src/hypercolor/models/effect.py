"""Effect models."""

from __future__ import annotations

from typing import Any

import msgspec

from .common import NamedRef, TransitionSpec


class ControlDefinition(msgspec.Struct, kw_only=True):
    """Dynamic effect control schema."""

    id: str
    label: str
    type: str
    default: Any = None
    value: Any = None
    min: float | None = None
    max: float | None = None
    step: float | None = None
    options: list[str] | None = None


class EffectPresetSummary(msgspec.Struct, kw_only=True):
    """A named preset for an effect."""

    name: str
    is_default: bool = False


class EffectSummary(msgspec.Struct, kw_only=True):
    """Effect data returned by list endpoints."""

    id: str
    name: str
    description: str
    author: str
    category: str
    source: str
    runnable: bool
    version: str
    audio_reactive: bool = False
    tags: list[str] = msgspec.field(default_factory=list)
    cover_image_url: str | None = None


class Effect(EffectSummary):
    """Full effect details including control schema."""

    controls: list[ControlDefinition] = msgspec.field(default_factory=list)
    presets: list[EffectPresetSummary] = msgspec.field(default_factory=list)
    active_control_values: dict[str, Any] | None = None


class ActiveEffect(msgspec.Struct, kw_only=True):
    """The currently active effect and its live control values."""

    id: str
    name: str
    state: str
    controls: list[ControlDefinition] = msgspec.field(default_factory=list)
    control_values: dict[str, Any] = msgspec.field(default_factory=dict)
    active_preset_id: str | None = None
    cover_image_url: str | None = None


class EffectCoverImage(msgspec.Struct, kw_only=True):
    """Binary cover image payload for an effect."""

    data: bytes
    content_type: str
    url: str


class ApplyEffectResult(msgspec.Struct, kw_only=True):
    """Response from applying an effect."""

    effect: NamedRef
    applied_controls: dict[str, Any] = msgspec.field(default_factory=dict)
    layout: dict[str, Any] | None = None
    transition: TransitionSpec | None = None


class ControlUpdateResult(msgspec.Struct, kw_only=True):
    """Response from updating current effect controls."""

    effect: str
    applied: dict[str, Any] = msgspec.field(default_factory=dict)
    rejected: list[str] = msgspec.field(default_factory=list)
