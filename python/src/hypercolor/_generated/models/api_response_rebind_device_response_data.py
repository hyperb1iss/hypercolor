from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

T = TypeVar("T", bound="ApiResponseRebindDeviceResponseData")


@_attrs_define
class ApiResponseRebindDeviceResponseData:
    """Response for `POST /api/v1/devices/rebind`.

    Attributes:
        device_id (str):
        layout_device_id (str): The layout binding id the device now resolves to.
        portable_key (str): The portable key that was re-pinned to the inherited identity.
    """

    device_id: str
    layout_device_id: str
    portable_key: str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        device_id = self.device_id

        layout_device_id = self.layout_device_id

        portable_key = self.portable_key

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "device_id": device_id,
                "layout_device_id": layout_device_id,
                "portable_key": portable_key,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        device_id = d.pop("device_id")

        layout_device_id = d.pop("layout_device_id")

        portable_key = d.pop("portable_key")

        api_response_rebind_device_response_data = cls(
            device_id=device_id,
            layout_device_id=layout_device_id,
            portable_key=portable_key,
        )

        api_response_rebind_device_response_data.additional_properties = d
        return api_response_rebind_device_response_data

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
