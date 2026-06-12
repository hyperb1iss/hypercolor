from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.broadcast_media_layer_group_response_items_item import (
        BroadcastMediaLayerGroupResponseItemsItem,
    )


T = TypeVar("T", bound="BroadcastMediaLayerGroupResponse")


@_attrs_define
class BroadcastMediaLayerGroupResponse:
    """
    Attributes:
        group_id (str):
        items (list[BroadcastMediaLayerGroupResponseItemsItem]):
        layers_version (int):
    """

    group_id: str
    items: list[BroadcastMediaLayerGroupResponseItemsItem]
    layers_version: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        group_id = self.group_id

        items = []
        for items_item_data in self.items:
            items_item = items_item_data.to_dict()
            items.append(items_item)

        layers_version = self.layers_version

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "group_id": group_id,
                "items": items,
                "layers_version": layers_version,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.broadcast_media_layer_group_response_items_item import (
            BroadcastMediaLayerGroupResponseItemsItem,
        )

        d = dict(src_dict)
        group_id = d.pop("group_id")

        items = []
        _items = d.pop("items")
        for items_item_data in _items:
            items_item = BroadcastMediaLayerGroupResponseItemsItem.from_dict(
                items_item_data
            )

            items.append(items_item)

        layers_version = d.pop("layers_version")

        broadcast_media_layer_group_response = cls(
            group_id=group_id,
            items=items,
            layers_version=layers_version,
        )

        broadcast_media_layer_group_response.additional_properties = d
        return broadcast_media_layer_group_response

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
