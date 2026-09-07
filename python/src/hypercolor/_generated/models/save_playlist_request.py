from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.playlist_item_request import PlaylistItemRequest


T = TypeVar("T", bound="SavePlaylistRequest")


@_attrs_define
class SavePlaylistRequest:
    """Request body for `POST /api/v1/library/playlists` and
    `PUT /api/v1/library/playlists/{id}`.

        Attributes:
            name (str):
            description (None | str | Unset):
            items (list[PlaylistItemRequest] | None | Unset):
            loop_enabled (bool | None | Unset):
    """

    name: str
    description: None | str | Unset = UNSET
    items: list[PlaylistItemRequest] | None | Unset = UNSET
    loop_enabled: bool | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        name = self.name

        description: None | str | Unset
        if isinstance(self.description, Unset):
            description = UNSET
        else:
            description = self.description

        items: list[dict[str, Any]] | None | Unset
        if isinstance(self.items, Unset):
            items = UNSET
        elif isinstance(self.items, list):
            items = []
            for items_type_0_item_data in self.items:
                items_type_0_item = items_type_0_item_data.to_dict()
                items.append(items_type_0_item)

        else:
            items = self.items

        loop_enabled: bool | None | Unset
        if isinstance(self.loop_enabled, Unset):
            loop_enabled = UNSET
        else:
            loop_enabled = self.loop_enabled

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "name": name,
            }
        )
        if description is not UNSET:
            field_dict["description"] = description
        if items is not UNSET:
            field_dict["items"] = items
        if loop_enabled is not UNSET:
            field_dict["loop_enabled"] = loop_enabled

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.playlist_item_request import PlaylistItemRequest

        d = dict(src_dict)
        name = d.pop("name")

        def _parse_description(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        description = _parse_description(d.pop("description", UNSET))

        def _parse_items(data: object) -> list[PlaylistItemRequest] | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, list):
                    raise TypeError()
                items_type_0 = []
                _items_type_0 = data
                for items_type_0_item_data in _items_type_0:
                    items_type_0_item = PlaylistItemRequest.from_dict(
                        items_type_0_item_data
                    )

                    items_type_0.append(items_type_0_item)

                return items_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(list[PlaylistItemRequest] | None | Unset, data)

        items = _parse_items(d.pop("items", UNSET))

        def _parse_loop_enabled(data: object) -> bool | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(bool | None | Unset, data)

        loop_enabled = _parse_loop_enabled(d.pop("loop_enabled", UNSET))

        save_playlist_request = cls(
            name=name,
            description=description,
            items=items,
            loop_enabled=loop_enabled,
        )

        save_playlist_request.additional_properties = d
        return save_playlist_request

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
