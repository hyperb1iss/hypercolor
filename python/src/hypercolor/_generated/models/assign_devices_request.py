from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.assign_devices_request_device_zones_item import (
        AssignDevicesRequestDeviceZonesItem,
    )


T = TypeVar("T", bound="AssignDevicesRequest")


@_attrs_define
class AssignDevicesRequest:
    """Request body for `POST /api/v1/scenes/{id}/zones/{zone_id}/devices`.

    Attributes:
        device_zones (list[AssignDevicesRequestDeviceZonesItem]):
    """

    device_zones: list[AssignDevicesRequestDeviceZonesItem]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        device_zones = []
        for device_zones_item_data in self.device_zones:
            device_zones_item = device_zones_item_data.to_dict()
            device_zones.append(device_zones_item)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "device_zones": device_zones,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.assign_devices_request_device_zones_item import (
            AssignDevicesRequestDeviceZonesItem,
        )

        d = dict(src_dict)
        device_zones = []
        _device_zones = d.pop("device_zones")
        for device_zones_item_data in _device_zones:
            device_zones_item = AssignDevicesRequestDeviceZonesItem.from_dict(
                device_zones_item_data
            )

            device_zones.append(device_zones_item)

        assign_devices_request = cls(
            device_zones=device_zones,
        )

        assign_devices_request.additional_properties = d
        return assign_devices_request

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
