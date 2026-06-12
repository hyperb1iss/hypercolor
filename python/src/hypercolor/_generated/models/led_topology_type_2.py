from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.led_topology_type_2_type import LedTopologyType2Type
from ..models.winding import Winding

T = TypeVar("T", bound="LedTopologyType2")


@_attrs_define
class LedTopologyType2:
    """LEDs arranged in a circle (fan ring, LED halo).

    When the zone is non-square, the ring becomes an ellipse.

        Attributes:
            count (int): Number of LEDs on the ring.
            direction (Winding): Winding direction for circular topologies.
            start_angle (float): Angle of LED 0 in radians. 0 = right (3 o'clock).
            type_ (LedTopologyType2Type):
    """

    count: int
    direction: Winding
    start_angle: float
    type_: LedTopologyType2Type
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        count = self.count

        direction = self.direction.value

        start_angle = self.start_angle

        type_ = self.type_.value

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "count": count,
                "direction": direction,
                "start_angle": start_angle,
                "type": type_,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        count = d.pop("count")

        direction = Winding(d.pop("direction"))

        start_angle = d.pop("start_angle")

        type_ = LedTopologyType2Type(d.pop("type"))

        led_topology_type_2 = cls(
            count=count,
            direction=direction,
            start_angle=start_angle,
            type_=type_,
        )

        led_topology_type_2.additional_properties = d
        return led_topology_type_2

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
