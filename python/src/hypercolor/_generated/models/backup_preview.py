from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast
from uuid import UUID

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

T = TypeVar("T", bound="BackupPreview")


@_attrs_define
class BackupPreview:
    """
    Attributes:
        effects (int):
        favorites (int):
        includes_preferences (bool):
        layouts (int):
        media (int):
        name (str):
        operation_id (UUID):
        scenes (int):
        unresolved_devices (list[str]):
    """

    effects: int
    favorites: int
    includes_preferences: bool
    layouts: int
    media: int
    name: str
    operation_id: UUID
    scenes: int
    unresolved_devices: list[str]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        effects = self.effects

        favorites = self.favorites

        includes_preferences = self.includes_preferences

        layouts = self.layouts

        media = self.media

        name = self.name

        operation_id = str(self.operation_id)

        scenes = self.scenes

        unresolved_devices = self.unresolved_devices

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "effects": effects,
                "favorites": favorites,
                "includes_preferences": includes_preferences,
                "layouts": layouts,
                "media": media,
                "name": name,
                "operation_id": operation_id,
                "scenes": scenes,
                "unresolved_devices": unresolved_devices,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        effects = d.pop("effects")

        favorites = d.pop("favorites")

        includes_preferences = d.pop("includes_preferences")

        layouts = d.pop("layouts")

        media = d.pop("media")

        name = d.pop("name")

        operation_id = UUID(d.pop("operation_id"))

        scenes = d.pop("scenes")

        unresolved_devices = cast(list[str], d.pop("unresolved_devices"))

        backup_preview = cls(
            effects=effects,
            favorites=favorites,
            includes_preferences=includes_preferences,
            layouts=layouts,
            media=media,
            name=name,
            operation_id=operation_id,
            scenes=scenes,
            unresolved_devices=unresolved_devices,
        )

        backup_preview.additional_properties = d
        return backup_preview

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
