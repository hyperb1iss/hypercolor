from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.zone_topology_summary_type_4_type import ZoneTopologySummaryType4Type

T = TypeVar("T", bound="ZoneTopologySummaryType4")


@_attrs_define
class ZoneTopologySummaryType4:
    """
    Attributes:
        circular (bool):
        height (int):
        type_ (ZoneTopologySummaryType4Type):
        width (int):
    """

    circular: bool
    height: int
    type_: ZoneTopologySummaryType4Type
    width: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        circular = self.circular

        height = self.height

        type_ = self.type_.value

        width = self.width

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "circular": circular,
                "height": height,
                "type": type_,
                "width": width,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        circular = d.pop("circular")

        height = d.pop("height")

        type_ = ZoneTopologySummaryType4Type(d.pop("type"))

        width = d.pop("width")

        zone_topology_summary_type_4 = cls(
            circular=circular,
            height=height,
            type_=type_,
            width=width,
        )

        zone_topology_summary_type_4.additional_properties = d
        return zone_topology_summary_type_4

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
