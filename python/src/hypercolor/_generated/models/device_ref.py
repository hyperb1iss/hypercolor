from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.device_origin import DeviceOrigin


T = TypeVar("T", bound="DeviceRef")


@_attrs_define
class DeviceRef:
    """Lightweight reference to a discovered device.

    Attributes:
        id (str):
        led_count (int):
        name (str):
        origin (DeviceOrigin): Origin metadata that separates device ownership from output routing.
    """

    id: str
    led_count: int
    name: str
    origin: DeviceOrigin
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        id = self.id

        led_count = self.led_count

        name = self.name

        origin = self.origin.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "id": id,
                "led_count": led_count,
                "name": name,
                "origin": origin,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.device_origin import DeviceOrigin

        d = dict(src_dict)
        id = d.pop("id")

        led_count = d.pop("led_count")

        name = d.pop("name")

        origin = DeviceOrigin.from_dict(d.pop("origin"))

        device_ref = cls(
            id=id,
            led_count=led_count,
            name=name,
            origin=origin,
        )

        device_ref.additional_properties = d
        return device_ref

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
