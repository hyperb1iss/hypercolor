from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.led_topology_type_6_type import LedTopologyType6Type

if TYPE_CHECKING:
    from ..models.normalized_position import NormalizedPosition


T = TypeVar("T", bound="LedTopologyType6")


@_attrs_define
class LedTopologyType6:
    """Arbitrary LED positions defined manually or imported.

    Positions are normalized `[0.0, 1.0]` within the zone bounding box.

        Attributes:
            positions (list[NormalizedPosition]): Directly-stored LED positions.
            type_ (LedTopologyType6Type):
    """

    positions: list[NormalizedPosition]
    type_: LedTopologyType6Type
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        positions = []
        for positions_item_data in self.positions:
            positions_item = positions_item_data.to_dict()
            positions.append(positions_item)

        type_ = self.type_.value

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "positions": positions,
                "type": type_,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.normalized_position import NormalizedPosition

        d = dict(src_dict)
        positions = []
        _positions = d.pop("positions")
        for positions_item_data in _positions:
            positions_item = NormalizedPosition.from_dict(positions_item_data)

            positions.append(positions_item)

        type_ = LedTopologyType6Type(d.pop("type"))

        led_topology_type_6 = cls(
            positions=positions,
            type_=type_,
        )

        led_topology_type_6.additional_properties = d
        return led_topology_type_6

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
