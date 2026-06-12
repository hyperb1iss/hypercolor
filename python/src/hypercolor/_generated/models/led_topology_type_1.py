from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.corner import Corner
from ..models.led_topology_type_1_type import LedTopologyType1Type

T = TypeVar("T", bound="LedTopologyType1")


@_attrs_define
class LedTopologyType1:
    """2D grid of LEDs (WLED matrix, Strimer, LED panel).

    Row-major indexing. The `serpentine` flag affects output buffer
    ordering only, NOT spatial positions.

        Attributes:
            height (int): Rows in the grid.
            serpentine (bool): Alternating row direction for serpentine wiring.
            start_corner (Corner): Corner for matrix start position.
            type_ (LedTopologyType1Type):
            width (int): Columns in the grid.
    """

    height: int
    serpentine: bool
    start_corner: Corner
    type_: LedTopologyType1Type
    width: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        height = self.height

        serpentine = self.serpentine

        start_corner = self.start_corner.value

        type_ = self.type_.value

        width = self.width

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "height": height,
                "serpentine": serpentine,
                "start_corner": start_corner,
                "type": type_,
                "width": width,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        height = d.pop("height")

        serpentine = d.pop("serpentine")

        start_corner = Corner(d.pop("start_corner"))

        type_ = LedTopologyType1Type(d.pop("type"))

        width = d.pop("width")

        led_topology_type_1 = cls(
            height=height,
            serpentine=serpentine,
            start_corner=start_corner,
            type_=type_,
            width=width,
        )

        led_topology_type_1.additional_properties = d
        return led_topology_type_1

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
