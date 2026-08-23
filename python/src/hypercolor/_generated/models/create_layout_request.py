from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="CreateLayoutRequest")


@_attrs_define
class CreateLayoutRequest:
    """Request body for `POST /api/v1/layouts`.

    Attributes:
        name (str):
        canvas_height (int | None | Unset):
        canvas_width (int | None | Unset):
        description (None | str | Unset):
    """

    name: str
    canvas_height: int | None | Unset = UNSET
    canvas_width: int | None | Unset = UNSET
    description: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        name = self.name

        canvas_height: int | None | Unset
        if isinstance(self.canvas_height, Unset):
            canvas_height = UNSET
        else:
            canvas_height = self.canvas_height

        canvas_width: int | None | Unset
        if isinstance(self.canvas_width, Unset):
            canvas_width = UNSET
        else:
            canvas_width = self.canvas_width

        description: None | str | Unset
        if isinstance(self.description, Unset):
            description = UNSET
        else:
            description = self.description

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "name": name,
            }
        )
        if canvas_height is not UNSET:
            field_dict["canvas_height"] = canvas_height
        if canvas_width is not UNSET:
            field_dict["canvas_width"] = canvas_width
        if description is not UNSET:
            field_dict["description"] = description

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        name = d.pop("name")

        def _parse_canvas_height(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        canvas_height = _parse_canvas_height(d.pop("canvas_height", UNSET))

        def _parse_canvas_width(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        canvas_width = _parse_canvas_width(d.pop("canvas_width", UNSET))

        def _parse_description(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        description = _parse_description(d.pop("description", UNSET))

        create_layout_request = cls(
            name=name,
            canvas_height=canvas_height,
            canvas_width=canvas_width,
            description=description,
        )

        create_layout_request.additional_properties = d
        return create_layout_request

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
