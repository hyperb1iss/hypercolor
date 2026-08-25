from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.driver_transport_availability_type_1_status import (
    DriverTransportAvailabilityType1Status,
)

T = TypeVar("T", bound="DriverTransportAvailabilityType1")


@_attrs_define
class DriverTransportAvailabilityType1:
    """No runtime backend exists on the current platform.

    Attributes:
        platform (str): Human-readable operating-system name.
        status (DriverTransportAvailabilityType1Status):
    """

    platform: str
    status: DriverTransportAvailabilityType1Status
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        platform = self.platform

        status = self.status.value

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "platform": platform,
                "status": status,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        platform = d.pop("platform")

        status = DriverTransportAvailabilityType1Status(d.pop("status"))

        driver_transport_availability_type_1 = cls(
            platform=platform,
            status=status,
        )

        driver_transport_availability_type_1.additional_properties = d
        return driver_transport_availability_type_1

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
