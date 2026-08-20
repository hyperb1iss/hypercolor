"""Attachment models."""

from __future__ import annotations

import msgspec


class AttachmentSlot(msgspec.Struct, kw_only=True):
    """One physical attachment point exposed by a controller."""

    id: str
    name: str
    led_start: int
    led_count: int
    suggested_categories: list[str] = msgspec.field(default_factory=list)
    allowed_templates: list[str] = msgspec.field(default_factory=list)
    allow_custom: bool = True


class AttachmentBinding(msgspec.Struct, kw_only=True):
    """One resolved attachment binding in a device summary."""

    slot_id: str
    template_id: str
    template_name: str
    enabled: bool
    instances: int
    led_offset: int
    effective_led_count: int
    name: str | None = None


class AttachmentSuggestedZone(msgspec.Struct, kw_only=True):
    """One attachment-derived spatial-zone suggestion."""

    slot_id: str
    template_id: str
    template_name: str
    name: str
    instance: int
    led_start: int
    led_count: int
    category: str
    default_size: dict[str, float]
    topology: dict[str, object]
    led_mapping: list[int] | None = None


class DeviceAttachments(msgspec.Struct, kw_only=True):
    """Resolved attachment state embedded in a device summary."""

    device_id: str
    device_name: str
    slots: list[AttachmentSlot] = msgspec.field(default_factory=list)
    bindings: list[AttachmentBinding] = msgspec.field(default_factory=list)
    suggested_zones: list[AttachmentSuggestedZone] = msgspec.field(default_factory=list)


class AttachmentTemplate(msgspec.Struct, kw_only=True):
    """Minimal attachment template summary."""

    id: str
    name: str
