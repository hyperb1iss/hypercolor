from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="CreateSimulatedDisplayRequest")


@_attrs_define
class CreateSimulatedDisplayRequest:
    """Request body for `POST /api/v1/simulators/displays`.

    Attributes:
        height (int):
        name (str):
        width (int):
        circular (bool | Unset):
        enabled (bool | None | Unset):
    """

    height: int
    name: str
    width: int
    circular: bool | Unset = UNSET
    enabled: bool | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        height = self.height

        name = self.name

        width = self.width

        circular = self.circular

        enabled: bool | None | Unset
        if isinstance(self.enabled, Unset):
            enabled = UNSET
        else:
            enabled = self.enabled

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "height": height,
                "name": name,
                "width": width,
            }
        )
        if circular is not UNSET:
            field_dict["circular"] = circular
        if enabled is not UNSET:
            field_dict["enabled"] = enabled

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        height = d.pop("height")

        name = d.pop("name")

        width = d.pop("width")

        circular = d.pop("circular", UNSET)

        def _parse_enabled(data: object) -> bool | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(bool | None | Unset, data)

        enabled = _parse_enabled(d.pop("enabled", UNSET))

        create_simulated_display_request = cls(
            height=height,
            name=name,
            width=width,
            circular=circular,
            enabled=enabled,
        )

        create_simulated_display_request.additional_properties = d
        return create_simulated_display_request

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
