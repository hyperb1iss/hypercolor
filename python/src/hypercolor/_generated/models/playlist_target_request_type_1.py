from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.playlist_target_request_type_1_type import PlaylistTargetRequestType1Type

T = TypeVar("T", bound="PlaylistTargetRequestType1")


@_attrs_define
class PlaylistTargetRequestType1:
    """
    Attributes:
        preset_id (str):
        type_ (PlaylistTargetRequestType1Type):
    """

    preset_id: str
    type_: PlaylistTargetRequestType1Type
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        preset_id = self.preset_id

        type_ = self.type_.value

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "preset_id": preset_id,
                "type": type_,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        preset_id = d.pop("preset_id")

        type_ = PlaylistTargetRequestType1Type(d.pop("type"))

        playlist_target_request_type_1 = cls(
            preset_id=preset_id,
            type_=type_,
        )

        playlist_target_request_type_1.additional_properties = d
        return playlist_target_request_type_1

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
