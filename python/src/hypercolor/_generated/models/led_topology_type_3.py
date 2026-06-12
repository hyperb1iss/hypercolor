from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.led_topology_type_3_type import LedTopologyType3Type

if TYPE_CHECKING:
    from ..models.ring_def import RingDef


T = TypeVar("T", bound="LedTopologyType3")


@_attrs_define
class LedTopologyType3:
    """Concentric rings (dual-ring fans like Corsair QL120).

    LEDs are emitted ring-by-ring (outermost first).

        Attributes:
            rings (list[RingDef]): Ring definitions from outermost to innermost.
            type_ (LedTopologyType3Type):
    """

    rings: list[RingDef]
    type_: LedTopologyType3Type
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        rings = []
        for rings_item_data in self.rings:
            rings_item = rings_item_data.to_dict()
            rings.append(rings_item)

        type_ = self.type_.value

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "rings": rings,
                "type": type_,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.ring_def import RingDef

        d = dict(src_dict)
        rings = []
        _rings = d.pop("rings")
        for rings_item_data in _rings:
            rings_item = RingDef.from_dict(rings_item_data)

            rings.append(rings_item)

        type_ = LedTopologyType3Type(d.pop("type"))

        led_topology_type_3 = cls(
            rings=rings,
            type_=type_,
        )

        led_topology_type_3.additional_properties = d
        return led_topology_type_3

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
