"""Driver module models."""

from __future__ import annotations

import msgspec

type TransportKind = str | dict[str, str]


class DriverTransportAvailable(
    msgspec.Struct,
    tag="available",
    tag_field="status",
    kw_only=True,
):
    """A driver transport backed on the current platform."""


class DriverTransportUnsupportedPlatform(
    msgspec.Struct,
    tag="unsupported_platform",
    tag_field="status",
    kw_only=True,
):
    """A driver transport without a backend on the current platform."""

    platform: str


type DriverTransportAvailability = DriverTransportAvailable | DriverTransportUnsupportedPlatform


class DriverTransportDescriptor(msgspec.Struct, kw_only=True):
    """One transport category and its current platform availability."""

    kind: TransportKind
    availability: DriverTransportAvailability


class DriverPresentation(msgspec.Struct, kw_only=True):
    """API and UI presentation metadata for a driver module."""

    label: str
    icon: str | None = None
    short_label: str | None = None
    accent_rgb: list[int] | None = None
    secondary_rgb: list[int] | None = None
    default_device_class: str | None = None


class DriverCapabilitySet(msgspec.Struct, kw_only=True):
    """Capability flags exposed by a driver module."""

    config: bool
    discovery: bool
    pairing: bool
    output_backend: bool
    protocol_catalog: bool
    runtime_cache: bool
    credentials: bool
    presentation: bool
    controls: bool


class DriverModuleDescriptor(msgspec.Struct, kw_only=True):
    """Stable module descriptor for native and future Wasm driver registries."""

    id: str
    display_name: str
    module_kind: str
    transports: list[DriverTransportDescriptor]
    capabilities: DriverCapabilitySet
    api_schema_version: int
    config_version: int
    default_enabled: bool
    vendor_name: str | None = None


class DriverProtocolDescriptor(msgspec.Struct, kw_only=True):
    """Protocol descriptor contributed by a driver module."""

    driver_id: str
    protocol_id: str
    display_name: str
    family_id: str
    transport: TransportKind
    route_backend_id: str
    vendor_id: int | None = None
    product_id: int | None = None
    model_id: str | None = None
    presentation: DriverPresentation | None = None


class Driver(msgspec.Struct, kw_only=True):
    """Registered driver module summary."""

    descriptor: DriverModuleDescriptor
    enabled: bool
    config_key: str
    presentation: DriverPresentation | None = None
    protocols: list[DriverProtocolDescriptor] = msgspec.field(default_factory=list)
    control_surface_id: str | None = None
    control_surface_path: str | None = None
