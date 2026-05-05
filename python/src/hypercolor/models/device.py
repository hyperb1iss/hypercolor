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
    zones: list[Zone]
    legacy_backend: str | None = msgspec.field(default=None, name="backend")
    origin: DeviceOrigin | None = None
    presentation: DevicePresentation | None = None
    connection: DeviceConnection | None = None
    firmware_version: str | None = None
    legacy_network_ip: str | None = msgspec.field(default=None, name="network_ip")
    legacy_network_hostname: str | None = msgspec.field(default=None, name="network_hostname")
    legacy_connection_label: str | None = msgspec.field(default=None, name="connection_label")
    auth: dict[str, Any] | None = None

    @property
    def backend(self) -> str:
        """Return the output backend ID for legacy callers."""

        if self.legacy_backend:
            return self.legacy_backend
        if self.origin is not None:
            return self.origin.backend_id or self.origin.driver_id or "unknown"
        return "unknown"

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
    def connection_label(self) -> str | None:
        """Return a human-readable connection label."""

        if self.legacy_connection_label:
            return self.legacy_connection_label
        if self.connection is not None:
            return self.connection.label or self.connection.endpoint
        return None

    @property
    def network_ip(self) -> str | None:
        """Return the network IP when the daemon exposes one."""

        if self.legacy_network_ip:
            return self.legacy_network_ip
        return self.connection.ip if self.connection is not None else None

    @property
    def network_hostname(self) -> str | None:
        """Return the network hostname when the daemon exposes one."""

        if self.legacy_network_hostname:
            return self.legacy_network_hostname
        return self.connection.hostname if self.connection is not None else None

    @property
    def enabled(self) -> bool:
        """Return whether the device output is enabled."""

        return self.status != "disabled"


class DeviceUpdate(msgspec.Struct, kw_only=True):
    """PUT body for device configuration changes."""

    name: str | None = None
    enabled: bool | None = None
    brightness: int | None = None
