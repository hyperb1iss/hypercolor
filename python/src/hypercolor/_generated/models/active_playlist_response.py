from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

T = TypeVar("T", bound="ActivePlaylistResponse")


@_attrs_define
class ActivePlaylistResponse:
    """The playlist the daemon is currently cycling through.

    This is the live runtime's view, not the stored playlist: the item
    list is reduced to `item_count`, and `started_at_ms` is when playback
    began rather than when the playlist was saved.

        Attributes:
            id (str):
            item_count (int):
            loop_enabled (bool):
            name (str):
            started_at_ms (int):
    """

    id: str
    item_count: int
    loop_enabled: bool
    name: str
    started_at_ms: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        id = self.id

        item_count = self.item_count

        loop_enabled = self.loop_enabled

        name = self.name

        started_at_ms = self.started_at_ms

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "id": id,
                "item_count": item_count,
                "loop_enabled": loop_enabled,
                "name": name,
                "started_at_ms": started_at_ms,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        id = d.pop("id")

        item_count = d.pop("item_count")

        loop_enabled = d.pop("loop_enabled")

        name = d.pop("name")

        started_at_ms = d.pop("started_at_ms")

        active_playlist_response = cls(
            id=id,
            item_count=item_count,
            loop_enabled=loop_enabled,
            name=name,
            started_at_ms=started_at_ms,
        )

        active_playlist_response.additional_properties = d
        return active_playlist_response

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
