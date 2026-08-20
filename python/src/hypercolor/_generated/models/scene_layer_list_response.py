from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.page_info import PageInfo
    from ..models.scene_layer import SceneLayer


T = TypeVar("T", bound="SceneLayerListResponse")


@_attrs_define
class SceneLayerListResponse:
    """Canonical list payload: honest pagination or none at all.

    `page: None` means the response is complete — no fabricated
    `limit`/`has_more` block pretending a paging contract that doesn't
    exist. `page: Some` means the endpoint genuinely pages.

        Attributes:
            items (list[SceneLayer]): The items.
            total (int): Total matching items (across all pages when paged).
            page (None | PageInfo | Unset):
    """

    items: list[SceneLayer]
    total: int
    page: None | PageInfo | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.page_info import PageInfo

        items = []
        for items_item_data in self.items:
            items_item = items_item_data.to_dict()
            items.append(items_item)

        total = self.total

        page: dict[str, Any] | None | Unset
        if isinstance(self.page, Unset):
            page = UNSET
        elif isinstance(self.page, PageInfo):
            page = self.page.to_dict()
        else:
            page = self.page

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "items": items,
                "total": total,
            }
        )
        if page is not UNSET:
            field_dict["page"] = page

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.page_info import PageInfo
        from ..models.scene_layer import SceneLayer

        d = dict(src_dict)
        items = []
        _items = d.pop("items")
        for items_item_data in _items:
            items_item = SceneLayer.from_dict(items_item_data)

            items.append(items_item)

        total = d.pop("total")

        def _parse_page(data: object) -> None | PageInfo | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                page_type_1 = PageInfo.from_dict(data)

                return page_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | PageInfo | Unset, data)

        page = _parse_page(d.pop("page", UNSET))

        scene_layer_list_response = cls(
            items=items,
            total=total,
            page=page,
        )

        scene_layer_list_response.additional_properties = d
        return scene_layer_list_response

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
