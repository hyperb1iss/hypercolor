from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.device_coverage_row import DeviceCoverageRow
    from ..models.driver_config_entry import DriverConfigEntry
    from ..models.open_rgb_endpoint_status import OpenRgbEndpointStatus
    from ..models.open_rgb_install_hint import OpenRgbInstallHint
    from ..models.open_rgb_permission_status import OpenRgbPermissionStatus


T = TypeVar("T", bound="OpenRgbStatus")


@_attrs_define
class OpenRgbStatus:
    """Host and bridge state for guided setup.

    Attributes:
        bridge_config (DriverConfigEntry): Host-owned wrapper around one driver's settings.
        compiled (bool):
        coverage (list[DeviceCoverageRow]):
        enabled (bool):
        install_hints (list[OpenRgbInstallHint]):
        output_disabled_count (int):
        permission_checks (list[OpenRgbPermissionStatus]):
        platform (str):
        probes (list[OpenRgbEndpointStatus]):
        binary_path (None | str | Unset):
        binary_version (None | str | Unset):
    """

    bridge_config: DriverConfigEntry
    compiled: bool
    coverage: list[DeviceCoverageRow]
    enabled: bool
    install_hints: list[OpenRgbInstallHint]
    output_disabled_count: int
    permission_checks: list[OpenRgbPermissionStatus]
    platform: str
    probes: list[OpenRgbEndpointStatus]
    binary_path: None | str | Unset = UNSET
    binary_version: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        bridge_config = self.bridge_config.to_dict()

        compiled = self.compiled

        coverage = []
        for coverage_item_data in self.coverage:
            coverage_item = coverage_item_data.to_dict()
            coverage.append(coverage_item)

        enabled = self.enabled

        install_hints = []
        for install_hints_item_data in self.install_hints:
            install_hints_item = install_hints_item_data.to_dict()
            install_hints.append(install_hints_item)

        output_disabled_count = self.output_disabled_count

        permission_checks = []
        for permission_checks_item_data in self.permission_checks:
            permission_checks_item = permission_checks_item_data.to_dict()
            permission_checks.append(permission_checks_item)

        platform = self.platform

        probes = []
        for probes_item_data in self.probes:
            probes_item = probes_item_data.to_dict()
            probes.append(probes_item)

        binary_path: None | str | Unset
        if isinstance(self.binary_path, Unset):
            binary_path = UNSET
        else:
            binary_path = self.binary_path

        binary_version: None | str | Unset
        if isinstance(self.binary_version, Unset):
            binary_version = UNSET
        else:
            binary_version = self.binary_version

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "bridge_config": bridge_config,
                "compiled": compiled,
                "coverage": coverage,
                "enabled": enabled,
                "install_hints": install_hints,
                "output_disabled_count": output_disabled_count,
                "permission_checks": permission_checks,
                "platform": platform,
                "probes": probes,
            }
        )
        if binary_path is not UNSET:
            field_dict["binary_path"] = binary_path
        if binary_version is not UNSET:
            field_dict["binary_version"] = binary_version

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.device_coverage_row import DeviceCoverageRow
        from ..models.driver_config_entry import DriverConfigEntry
        from ..models.open_rgb_endpoint_status import OpenRgbEndpointStatus
        from ..models.open_rgb_install_hint import OpenRgbInstallHint
        from ..models.open_rgb_permission_status import OpenRgbPermissionStatus

        d = dict(src_dict)
        bridge_config = DriverConfigEntry.from_dict(d.pop("bridge_config"))

        compiled = d.pop("compiled")

        coverage = []
        _coverage = d.pop("coverage")
        for coverage_item_data in _coverage:
            coverage_item = DeviceCoverageRow.from_dict(coverage_item_data)

            coverage.append(coverage_item)

        enabled = d.pop("enabled")

        install_hints = []
        _install_hints = d.pop("install_hints")
        for install_hints_item_data in _install_hints:
            install_hints_item = OpenRgbInstallHint.from_dict(install_hints_item_data)

            install_hints.append(install_hints_item)

        output_disabled_count = d.pop("output_disabled_count")

        permission_checks = []
        _permission_checks = d.pop("permission_checks")
        for permission_checks_item_data in _permission_checks:
            permission_checks_item = OpenRgbPermissionStatus.from_dict(
                permission_checks_item_data
            )

            permission_checks.append(permission_checks_item)

        platform = d.pop("platform")

        probes = []
        _probes = d.pop("probes")
        for probes_item_data in _probes:
            probes_item = OpenRgbEndpointStatus.from_dict(probes_item_data)

            probes.append(probes_item)

        def _parse_binary_path(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        binary_path = _parse_binary_path(d.pop("binary_path", UNSET))

        def _parse_binary_version(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        binary_version = _parse_binary_version(d.pop("binary_version", UNSET))

        open_rgb_status = cls(
            bridge_config=bridge_config,
            compiled=compiled,
            coverage=coverage,
            enabled=enabled,
            install_hints=install_hints,
            output_disabled_count=output_disabled_count,
            permission_checks=permission_checks,
            platform=platform,
            probes=probes,
            binary_path=binary_path,
            binary_version=binary_version,
        )

        open_rgb_status.additional_properties = d
        return open_rgb_status

    @property
    def additional_keys(self) -> list[str]:
        return list(self.additional_properties.keys())

    def __getitem__(self, key: str) -> Any:
        return self.additional_properties[key]

    def __setitem__(self, key: str, value: Any) -> None:
        self.additional_properties[key] = value

    def __delitem__(self, key: str) -> None:
        del self.additional_properties[key]

    def __contains__(self, key: str) -> bool:
        return key in self.additional_properties
