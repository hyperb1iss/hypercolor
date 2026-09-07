from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="LayoutSummary")


@_attrs_define
class LayoutSummary:
    """Summary row from `GET /api/v1/layouts`.

    Attributes:
        canvas_height (int):
        canvas_width (int):
        id (str):
        name (str):
        zone_count (int):
        is_active (bool | Unset):
    """

    canvas_height: int
    canvas_width: int
    id: str
    name: str
    zone_count: int
    is_active: bool | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        canvas_height = self.canvas_height

        canvas_width = self.canvas_width

        id = self.id

        name = self.name

        zone_count = self.zone_count

        is_active = self.is_active

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "canvas_height": canvas_height,
                "canvas_width": canvas_width,
                "id": id,
                "name": name,
                "zone_count": zone_count,
            }
        )
        if is_active is not UNSET:
            field_dict["is_active"] = is_active

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        canvas_height = d.pop("canvas_height")

        canvas_width = d.pop("canvas_width")

        id = d.pop("id")

        name = d.pop("name")

        zone_count = d.pop("zone_count")

        is_active = d.pop("is_active", UNSET)

        layout_summary = cls(
            canvas_height=canvas_height,
            canvas_width=canvas_width,
            id=id,
            name=name,
            zone_count=zone_count,
            is_active=is_active,
        )

        layout_summary.additional_properties = d
        return layout_summary

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
