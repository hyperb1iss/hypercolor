from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define

if TYPE_CHECKING:
    from ..models.member_placement import MemberPlacement


T = TypeVar("T", bound="ZoneLayoutRequest")


@_attrs_define
class ZoneLayoutRequest:
    """`PUT /scene/zones/{zone}/layout` — zone-scoped spatial override,
    in the same compact shape the zone resource reads back.

        Attributes:
            placements (list[MemberPlacement]):
    """

    placements: list[MemberPlacement]

    def to_dict(self) -> dict[str, Any]:
        placements = []
        for placements_item_data in self.placements:
            placements_item = placements_item_data.to_dict()
            placements.append(placements_item)

        field_dict: dict[str, Any] = {}

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

        zone_layout_request = cls(
            placements=placements,
        )

        return zone_layout_request
