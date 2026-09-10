from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.member_placement_hint import MemberPlacementHint


T = TypeVar("T", bound="MemberAssignmentTarget")


@_attrs_define
class MemberAssignmentTarget:
    """Device segments to assign using daemon-owned layout construction.

    Attributes:
        device_id (str):
        zone_id (str):
        placements (list[MemberPlacementHint] | Unset): Optional seeded geometry for newly minted outputs, keyed by
            segment.
        segments (list[str] | Unset): Empty selects every light segment, including all attachment instances.
    """

    device_id: str
    zone_id: str
    placements: list[MemberPlacementHint] | Unset = UNSET
    segments: list[str] | Unset = UNSET

    def to_dict(self) -> dict[str, Any]:
        device_id = self.device_id

        zone_id = self.zone_id

        placements: list[dict[str, Any]] | Unset = UNSET
        if not isinstance(self.placements, Unset):
            placements = []
            for placements_item_data in self.placements:
                placements_item = placements_item_data.to_dict()
                placements.append(placements_item)

        segments: list[str] | Unset = UNSET
        if not isinstance(self.segments, Unset):
            segments = self.segments

        field_dict: dict[str, Any] = {}

        field_dict.update(
            {
                "device_id": device_id,
                "zone_id": zone_id,
            }
        )
        if placements is not UNSET:
            field_dict["placements"] = placements
        if segments is not UNSET:
            field_dict["segments"] = segments

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.member_placement_hint import MemberPlacementHint

        d = dict(src_dict)
        device_id = d.pop("device_id")

        zone_id = d.pop("zone_id")

        _placements = d.pop("placements", UNSET)
        placements: list[MemberPlacementHint] | Unset = UNSET
        if _placements is not UNSET:
            placements = []
            for placements_item_data in _placements:
                placements_item = MemberPlacementHint.from_dict(placements_item_data)

                placements.append(placements_item)

        segments = cast(list[str], d.pop("segments", UNSET))

        member_assignment_target = cls(
            device_id=device_id,
            zone_id=zone_id,
            placements=placements,
            segments=segments,
        )

        return member_assignment_target
