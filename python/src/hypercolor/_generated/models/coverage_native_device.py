from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

T = TypeVar("T", bound="CoverageNativeDevice")


@_attrs_define
class CoverageNativeDevice:
    """The native side of a coverage row.

    Attributes:
        device_id (str):
        driver_id (str):
        state (str): Lifecycle state name in lowercase (`connected`, `known`, ...).
    """

    device_id: str
    driver_id: str
    state: str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        device_id = self.device_id

        driver_id = self.driver_id

        state = self.state

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "device_id": device_id,
                "driver_id": driver_id,
                "state": state,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        device_id = d.pop("device_id")

        driver_id = d.pop("driver_id")

        state = d.pop("state")

        coverage_native_device = cls(
            device_id=device_id,
            driver_id=driver_id,
            state=state,
        )

        coverage_native_device.additional_properties = d
        return coverage_native_device

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
