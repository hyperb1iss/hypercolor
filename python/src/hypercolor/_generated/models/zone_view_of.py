from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

T = TypeVar("T", bound="ZoneViewOf")


@_attrs_define
class ZoneViewOf:
    """Where a screen's view comes from: the zone that places the screen and
    the screen's membership in it.

        Attributes:
            member (str): A zone membership's identity — wire-transparent, unique within its
                zone, which is all its zone-scoped route needs.
            zone (str):
    """

    member: str
    zone: str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        member = self.member

        zone = self.zone

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "member": member,
                "zone": zone,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        member = d.pop("member")

        zone = d.pop("zone")

        zone_view_of = cls(
            member=member,
            zone=zone,
        )

        zone_view_of.additional_properties = d
        return zone_view_of

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
