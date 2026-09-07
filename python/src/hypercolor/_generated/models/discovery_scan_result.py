from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.device_ref import DeviceRef
    from ..models.discovery_scanner_result import DiscoveryScannerResult


T = TypeVar("T", bound="DiscoveryScanResult")


@_attrs_define
class DiscoveryScanResult:
    """Detailed result from a completed discovery scan.

    Attributes:
        duration_ms (int):
        new_devices (list[DeviceRef]):
        reappeared_devices (list[DeviceRef]):
        scanners (list[DiscoveryScannerResult]):
        targets (list[str]):
        timeout_ms (int):
        total_known (int):
        vanished_devices (list[str]):
    """

    duration_ms: int
    new_devices: list[DeviceRef]
    reappeared_devices: list[DeviceRef]
    scanners: list[DiscoveryScannerResult]
    targets: list[str]
    timeout_ms: int
    total_known: int
    vanished_devices: list[str]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        duration_ms = self.duration_ms

        new_devices = []
        for new_devices_item_data in self.new_devices:
            new_devices_item = new_devices_item_data.to_dict()
            new_devices.append(new_devices_item)

        reappeared_devices = []
        for reappeared_devices_item_data in self.reappeared_devices:
            reappeared_devices_item = reappeared_devices_item_data.to_dict()
            reappeared_devices.append(reappeared_devices_item)

        scanners = []
        for scanners_item_data in self.scanners:
            scanners_item = scanners_item_data.to_dict()
            scanners.append(scanners_item)

        targets = self.targets

        timeout_ms = self.timeout_ms

        total_known = self.total_known

        vanished_devices = self.vanished_devices

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "duration_ms": duration_ms,
                "new_devices": new_devices,
                "reappeared_devices": reappeared_devices,
                "scanners": scanners,
                "targets": targets,
                "timeout_ms": timeout_ms,
                "total_known": total_known,
                "vanished_devices": vanished_devices,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.device_ref import DeviceRef
        from ..models.discovery_scanner_result import DiscoveryScannerResult

        d = dict(src_dict)
        duration_ms = d.pop("duration_ms")

        new_devices = []
        _new_devices = d.pop("new_devices")
        for new_devices_item_data in _new_devices:
            new_devices_item = DeviceRef.from_dict(new_devices_item_data)

            new_devices.append(new_devices_item)

        reappeared_devices = []
        _reappeared_devices = d.pop("reappeared_devices")
        for reappeared_devices_item_data in _reappeared_devices:
            reappeared_devices_item = DeviceRef.from_dict(reappeared_devices_item_data)

            reappeared_devices.append(reappeared_devices_item)

        scanners = []
        _scanners = d.pop("scanners")
        for scanners_item_data in _scanners:
            scanners_item = DiscoveryScannerResult.from_dict(scanners_item_data)

            scanners.append(scanners_item)

        targets = cast(list[str], d.pop("targets"))

        timeout_ms = d.pop("timeout_ms")

        total_known = d.pop("total_known")

        vanished_devices = cast(list[str], d.pop("vanished_devices"))

        discovery_scan_result = cls(
            duration_ms=duration_ms,
            new_devices=new_devices,
            reappeared_devices=reappeared_devices,
            scanners=scanners,
            targets=targets,
            timeout_ms=timeout_ms,
            total_known=total_known,
            vanished_devices=vanished_devices,
        )

        discovery_scan_result.additional_properties = d
        return discovery_scan_result

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
