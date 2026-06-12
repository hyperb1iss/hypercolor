from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.zone_response_zone import ZoneResponseZone


T = TypeVar("T", bound="ZoneResponse")


@_attrs_define
class ZoneResponse:
    """Response carrying one zone after a create/get/update.

    Attributes:
        groups_revision (int):
        zone (ZoneResponseZone):
    """

    groups_revision: int
    zone: ZoneResponseZone
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        groups_revision = self.groups_revision

        zone = self.zone.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "groups_revision": groups_revision,
                "zone": zone,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.zone_response_zone import ZoneResponseZone

        d = dict(src_dict)
        groups_revision = d.pop("groups_revision")

        zone = ZoneResponseZone.from_dict(d.pop("zone"))

        zone_response = cls(
            groups_revision=groups_revision,
            zone=zone,
        )

        zone_response.additional_properties = d
        return zone_response

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
