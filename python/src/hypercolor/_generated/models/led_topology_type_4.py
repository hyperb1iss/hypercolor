from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.corner import Corner
from ..models.led_topology_type_4_type import LedTopologyType4Type
from ..models.winding import Winding

T = TypeVar("T", bound="LedTopologyType4")


@_attrs_define
class LedTopologyType4:
    """Rectangular perimeter loop (monitor backlight, ambilight-style).

    LEDs trace the rectangular perimeter of the zone.

        Attributes:
            bottom (int): LED count on bottom edge.
            direction (Winding): Winding direction for circular topologies.
            left (int): LED count on left edge.
            right (int): LED count on right edge.
            start_corner (Corner): Corner for matrix start position.
            top (int): LED count on top edge.
            type_ (LedTopologyType4Type):
    """

    bottom: int
    direction: Winding
    left: int
    right: int
    start_corner: Corner
    top: int
    type_: LedTopologyType4Type
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        bottom = self.bottom

        direction = self.direction.value

        left = self.left

        right = self.right

        start_corner = self.start_corner.value

        top = self.top

        type_ = self.type_.value

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "bottom": bottom,
                "direction": direction,
                "left": left,
                "right": right,
                "start_corner": start_corner,
                "top": top,
                "type": type_,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        bottom = d.pop("bottom")

        direction = Winding(d.pop("direction"))

        left = d.pop("left")

        right = d.pop("right")

        start_corner = Corner(d.pop("start_corner"))

        top = d.pop("top")

        type_ = LedTopologyType4Type(d.pop("type"))

        led_topology_type_4 = cls(
            bottom=bottom,
            direction=direction,
            left=left,
            right=right,
            start_corner=start_corner,
            top=top,
            type_=type_,
        )

        led_topology_type_4.additional_properties = d
        return led_topology_type_4

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
