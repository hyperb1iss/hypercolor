from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..types import UNSET, Unset

T = TypeVar("T", bound="InstalledEffectResponse")


@_attrs_define
class InstalledEffectResponse:
    """Response for `POST /api/v1/effects/install`.

    Attributes:
        controls (int):
        id (str):
        name (str):
        path (str):
        presets (int):
        console_after (int | Unset): Effect console cursor taken just before the install. Pass it as
            `after` to `GET /api/v1/effects/console` to read only what the new
            page prints.
    """

    controls: int
    id: str
    name: str
    path: str
    presets: int
    console_after: int | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        controls = self.controls

        id = self.id

        name = self.name

        path = self.path

        presets = self.presets

        console_after = self.console_after

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "controls": controls,
                "id": id,
                "name": name,
                "path": path,
                "presets": presets,
            }
        )
        if console_after is not UNSET:
            field_dict["console_after"] = console_after

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        controls = d.pop("controls")

        id = d.pop("id")

        name = d.pop("name")

        path = d.pop("path")

        presets = d.pop("presets")

        console_after = d.pop("console_after", UNSET)

        installed_effect_response = cls(
            controls=controls,
            id=id,
            name=name,
            path=path,
            presets=presets,
            console_after=console_after,
        )

        installed_effect_response.additional_properties = d
        return installed_effect_response

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
