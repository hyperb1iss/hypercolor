"""Device models."""

from __future__ import annotations

from typing import Any

import msgspec


class Zone(msgspec.Struct, kw_only=True):
    """A logical lighting zone within a device."""

    id: str
    name: str
    led_count: int
    topology: str
    topology_hint: dict[str, Any] | None = None


class Device(msgspec.Struct, kw_only=True):
    """A discovered or configured RGB device."""

    id: str
    layout_device_id: str
    name: str
    backend: str
    status: str
    brightness: int
    total_leds: int
    zones: list[Zone]
    firmware_version: str | None = None
    network_ip: str | None = None
    network_hostname: str | None = None
    connection_label: str | None = None
    auth: dict[str, Any] | None = None

    @property
    def enabled(self) -> bool:
        """Return whether the device output is enabled."""

        return self.status != "disabled"


class DeviceUpdate(msgspec.Struct, kw_only=True):
    """PUT body for device configuration changes."""

    name: str | None = None
    enabled: bool | None = None
    brightness: int | None = None
