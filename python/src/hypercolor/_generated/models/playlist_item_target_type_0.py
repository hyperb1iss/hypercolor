from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar
from uuid import UUID

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.playlist_item_target_type_0_type import PlaylistItemTargetType0Type

T = TypeVar("T", bound="PlaylistItemTargetType0")


@_attrs_define
class PlaylistItemTargetType0:
    """Run an effect directly.

    Attributes:
        effect_id (UUID): Unique identifier for an effect, wrapping a UUID v7.

            Generated at discovery time and used as the primary key across
            the registry, event bus, API, and UI.
        type_ (PlaylistItemTargetType0Type):
    """

    effect_id: UUID
    type_: PlaylistItemTargetType0Type
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        effect_id = str(self.effect_id)

        type_ = self.type_.value

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "effect_id": effect_id,
                "type": type_,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        effect_id = UUID(d.pop("effect_id"))

        type_ = PlaylistItemTargetType0Type(d.pop("type"))

        playlist_item_target_type_0 = cls(
            effect_id=effect_id,
            type_=type_,
        )

        playlist_item_target_type_0.additional_properties = d
        return playlist_item_target_type_0

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
