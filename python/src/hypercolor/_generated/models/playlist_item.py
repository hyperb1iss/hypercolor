from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast
from uuid import UUID

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.playlist_item_target_type_0 import PlaylistItemTargetType0
    from ..models.playlist_item_target_type_1 import PlaylistItemTargetType1


T = TypeVar("T", bound="PlaylistItem")


@_attrs_define
class PlaylistItem:
    """One item in a playlist sequence.

    Attributes:
        id (UUID): Opaque identifier for a playlist item.
        target (PlaylistItemTargetType0 | PlaylistItemTargetType1): Target entity for one playlist slot.
        duration_ms (int | None | Unset):
        transition_ms (int | None | Unset):
    """

    id: UUID
    target: PlaylistItemTargetType0 | PlaylistItemTargetType1
    duration_ms: int | None | Unset = UNSET
    transition_ms: int | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.playlist_item_target_type_0 import PlaylistItemTargetType0

        id = str(self.id)

        target: dict[str, Any]
        if isinstance(self.target, PlaylistItemTargetType0):
            target = self.target.to_dict()
        else:
            target = self.target.to_dict()

        duration_ms: int | None | Unset
        if isinstance(self.duration_ms, Unset):
            duration_ms = UNSET
        else:
            duration_ms = self.duration_ms

        transition_ms: int | None | Unset
        if isinstance(self.transition_ms, Unset):
            transition_ms = UNSET
        else:
            transition_ms = self.transition_ms

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "id": id,
                "target": target,
            }
        )
        if duration_ms is not UNSET:
            field_dict["duration_ms"] = duration_ms
        if transition_ms is not UNSET:
            field_dict["transition_ms"] = transition_ms

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.playlist_item_target_type_0 import PlaylistItemTargetType0
        from ..models.playlist_item_target_type_1 import PlaylistItemTargetType1

        d = dict(src_dict)
        id = UUID(d.pop("id"))

        def _parse_target(
            data: object,
        ) -> PlaylistItemTargetType0 | PlaylistItemTargetType1:
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_playlist_item_target_type_0 = (
                    PlaylistItemTargetType0.from_dict(data)
                )

                return componentsschemas_playlist_item_target_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            if not isinstance(data, dict):
                raise TypeError()
            componentsschemas_playlist_item_target_type_1 = (
                PlaylistItemTargetType1.from_dict(data)
            )

            return componentsschemas_playlist_item_target_type_1

        target = _parse_target(d.pop("target"))

        def _parse_duration_ms(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        duration_ms = _parse_duration_ms(d.pop("duration_ms", UNSET))

        def _parse_transition_ms(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        transition_ms = _parse_transition_ms(d.pop("transition_ms", UNSET))

        playlist_item = cls(
            id=id,
            target=target,
            duration_ms=duration_ms,
            transition_ms=transition_ms,
        )

        playlist_item.additional_properties = d
        return playlist_item

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
