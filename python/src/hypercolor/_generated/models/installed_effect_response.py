from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

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
    """

    controls: int
    id: str
    name: str
    path: str
    presets: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        controls = self.controls

        id = self.id

        name = self.name

        path = self.path

        presets = self.presets

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

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        controls = d.pop("controls")

        id = d.pop("id")

        name = d.pop("name")

        path = d.pop("path")

        presets = d.pop("presets")

        installed_effect_response = cls(
            controls=controls,
            id=id,
            name=name,
            path=path,
            presets=presets,
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
