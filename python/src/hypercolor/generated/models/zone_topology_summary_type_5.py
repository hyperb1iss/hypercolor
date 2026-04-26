from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.zone_topology_summary_type_5_type import ZoneTopologySummaryType5Type

T = TypeVar("T", bound="ZoneTopologySummaryType5")


@_attrs_define
class ZoneTopologySummaryType5:
    """
    Attributes:
        type_ (ZoneTopologySummaryType5Type):
    """

    type_: ZoneTopologySummaryType5Type
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        type_ = self.type_.value

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "type": type_,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        type_ = ZoneTopologySummaryType5Type(d.pop("type"))

        zone_topology_summary_type_5 = cls(
            type_=type_,
        )

        zone_topology_summary_type_5.additional_properties = d
        return zone_topology_summary_type_5

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
