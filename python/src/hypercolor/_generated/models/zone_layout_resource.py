from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.member_placement import MemberPlacement


T = TypeVar("T", bound="ZoneLayoutResource")


@_attrs_define
class ZoneLayoutResource:
    """The zone-scoped layout on the wire — the same shape written by
    `PUT .../layout` and read back on the zone resource. Compact by
    design: member placements only, no computed LED data, and no
    device-internal vocabulary (Spec 78 §5.1). Vec order is z-order.

        Attributes:
            placements (list[MemberPlacement]):
    """

    placements: list[MemberPlacement]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        placements = []
        for placements_item_data in self.placements:
            placements_item = placements_item_data.to_dict()
            placements.append(placements_item)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "placements": placements,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.member_placement import MemberPlacement

        d = dict(src_dict)
        placements = []
        _placements = d.pop("placements")
        for placements_item_data in _placements:
            placements_item = MemberPlacement.from_dict(placements_item_data)

            placements.append(placements_item)

        zone_layout_resource = cls(
            placements=placements,
        )

        zone_layout_resource.additional_properties = d
        return zone_layout_resource

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
