from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.output import Output


T = TypeVar("T", bound="UpdateLayoutRequest")


@_attrs_define
class UpdateLayoutRequest:
    """Request body for `PUT /api/v1/layouts/{id}`.

    Omitted fields leave the stored layout untouched; a present `zones`
    list replaces the layout's outputs wholesale.

        Attributes:
            canvas_height (int | None | Unset):
            canvas_width (int | None | Unset):
            description (None | str | Unset):
            name (None | str | Unset):
            zones (list[Output] | None | Unset):
    """

    canvas_height: int | None | Unset = UNSET
    canvas_width: int | None | Unset = UNSET
    description: None | str | Unset = UNSET
    name: None | str | Unset = UNSET
    zones: list[Output] | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
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

        name: None | str | Unset
        if isinstance(self.name, Unset):
            name = UNSET
        else:
            name = self.name

        zones: list[dict[str, Any]] | None | Unset
        if isinstance(self.zones, Unset):
            zones = UNSET
        elif isinstance(self.zones, list):
            zones = []
            for zones_type_0_item_data in self.zones:
                zones_type_0_item = zones_type_0_item_data.to_dict()
                zones.append(zones_type_0_item)

        else:
            zones = self.zones

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if canvas_height is not UNSET:
            field_dict["canvas_height"] = canvas_height
        if canvas_width is not UNSET:
            field_dict["canvas_width"] = canvas_width
        if description is not UNSET:
            field_dict["description"] = description
        if name is not UNSET:
            field_dict["name"] = name
        if zones is not UNSET:
            field_dict["zones"] = zones

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.output import Output

        d = dict(src_dict)

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

        def _parse_name(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        name = _parse_name(d.pop("name", UNSET))

        def _parse_zones(data: object) -> list[Output] | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, list):
                    raise TypeError()
                zones_type_0 = []
                _zones_type_0 = data
                for zones_type_0_item_data in _zones_type_0:
                    zones_type_0_item = Output.from_dict(zones_type_0_item_data)

                    zones_type_0.append(zones_type_0_item)

                return zones_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(list[Output] | None | Unset, data)

        zones = _parse_zones(d.pop("zones", UNSET))

        update_layout_request = cls(
            canvas_height=canvas_height,
            canvas_width=canvas_width,
            description=description,
            name=name,
            zones=zones,
        )

        update_layout_request.additional_properties = d
        return update_layout_request

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
