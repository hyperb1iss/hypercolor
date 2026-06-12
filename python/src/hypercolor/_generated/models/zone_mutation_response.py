from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.zone_mutation_response_items_item import ZoneMutationResponseItemsItem


T = TypeVar("T", bound="ZoneMutationResponse")


@_attrs_define
class ZoneMutationResponse:
    """Response carrying the full zone set after a bulk mutation.

    Attributes:
        groups_revision (int):
        items (list[ZoneMutationResponseItemsItem]):
    """

    groups_revision: int
    items: list[ZoneMutationResponseItemsItem]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        groups_revision = self.groups_revision

        items = []
        for items_item_data in self.items:
            items_item = items_item_data.to_dict()
            items.append(items_item)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "groups_revision": groups_revision,
                "items": items,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.zone_mutation_response_items_item import (
            ZoneMutationResponseItemsItem,
        )

        d = dict(src_dict)
        groups_revision = d.pop("groups_revision")

        items = []
        _items = d.pop("items")
        for items_item_data in _items:
            items_item = ZoneMutationResponseItemsItem.from_dict(items_item_data)

            items.append(items_item)

        zone_mutation_response = cls(
            groups_revision=groups_revision,
            items=items,
        )

        zone_mutation_response.additional_properties = d
        return zone_mutation_response

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
