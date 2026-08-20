"""Device models."""

from __future__ import annotations

from typing import Any

import msgspec

from .attachment import DeviceAttachments


class DeviceSegment(msgspec.Struct, kw_only=True):
    """One addressable hardware segment exposed by a device."""

    id: str
    name: str
    led_count: int
    topology: str
    topology_hint: dict[str, Any] | None = None


class DeviceOrigin(msgspec.Struct, kw_only=True):
    """Where a device came from inside the daemon."""

    driver_id: str | None = None
    backend_id: str | None = None
    transport: str | None = None
    protocol_id: str | None = None


class DevicePresentation(msgspec.Struct, kw_only=True):
    """Display hints exposed by a device driver."""

    label: str | None = None
    short_label: str | None = None
    accent_rgb: list[int] | None = None
    secondary_rgb: list[int] | None = None
    icon: str | None = None
    default_device_class: str | None = None


class DeviceConnection(msgspec.Struct, kw_only=True):
    """Current connection details for a device."""

    transport: str | None = None
    label: str | None = None
    endpoint: str | None = None
    ip: str | None = None
    hostname: str | None = None


class Device(msgspec.Struct, kw_only=True):
    """A discovered or configured RGB device."""

    id: str
    layout_device_id: str
    name: str
    status: str
    brightness: int
    total_leds: int
    segments: list[DeviceSegment]
    origin: DeviceOrigin | None = None
    presentation: DevicePresentation | None = None
    connection: DeviceConnection | None = None
    firmware_version: str | None = None
    auth: dict[str, Any] | None = None
    attachments: DeviceAttachments | None = None

    @property
    def driver_id(self) -> str | None:
        """Return the driver that discovered this device."""

        return self.origin.driver_id if self.origin is not None else None

    @property
    def transport(self) -> str | None:
        """Return the connection transport."""

        if self.connection is not None and self.connection.transport:
            return self.connection.transport
        return self.origin.transport if self.origin is not None else None

    @property
    def enabled(self) -> bool:
        """Return whether the device output is enabled."""

        return self.status != "disabled"


class DeviceUpdate(msgspec.Struct, kw_only=True):
    """PUT body for device configuration changes."""

    name: str | None = None
    enabled: bool | None = None
    brightness: int | None = None
