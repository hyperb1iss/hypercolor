from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.wall import Wall

T = TypeVar("T", bound="RoomAdjacency")


@_attrs_define
class RoomAdjacency:
    """Declares adjacency between two rooms for cross-room effects.

    Attributes:
        blend_width (int): Canvas pixels for cross-room blending zone.
        neighbor_id (str): ID of the neighboring space.
        shared_wall (Wall): Cardinal wall for room adjacency.
    """

    blend_width: int
    neighbor_id: str
    shared_wall: Wall
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        blend_width = self.blend_width

        neighbor_id = self.neighbor_id

        shared_wall = self.shared_wall.value

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "blend_width": blend_width,
                "neighbor_id": neighbor_id,
                "shared_wall": shared_wall,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        blend_width = d.pop("blend_width")

        neighbor_id = d.pop("neighbor_id")

        shared_wall = Wall(d.pop("shared_wall"))

        room_adjacency = cls(
            blend_width=blend_width,
            neighbor_id=neighbor_id,
            shared_wall=shared_wall,
        )

        room_adjacency.additional_properties = d
        return room_adjacency

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
