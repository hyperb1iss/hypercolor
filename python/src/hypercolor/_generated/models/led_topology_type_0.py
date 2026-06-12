from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.led_topology_type_0_type import LedTopologyType0Type
from ..models.strip_direction import StripDirection

T = TypeVar("T", bound="LedTopologyType0")


@_attrs_define
class LedTopologyType0:
    """Linear strip: LEDs in a straight line across the zone.

    The strip runs along one axis; the perpendicular axis is fixed at 0.5
    (the zone midline).

        Attributes:
            count (int): Total number of LEDs.
            direction (StripDirection): Direction for strip LED indexing.
            type_ (LedTopologyType0Type):
    """

    count: int
    direction: StripDirection
    type_: LedTopologyType0Type
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        count = self.count

        direction = self.direction.value

        type_ = self.type_.value

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "count": count,
                "direction": direction,
                "type": type_,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        count = d.pop("count")

        direction = StripDirection(d.pop("direction"))

        type_ = LedTopologyType0Type(d.pop("type"))

        led_topology_type_0 = cls(
            count=count,
            direction=direction,
            type_=type_,
        )

        led_topology_type_0.additional_properties = d
        return led_topology_type_0

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
