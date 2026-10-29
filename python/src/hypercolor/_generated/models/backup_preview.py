from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast
from uuid import UUID

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.missing_setup_resources import MissingSetupResources


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
        missing_resources (MissingSetupResources | Unset): Referenced content that must be installed or imported on this
            computer.
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
    missing_resources: MissingSetupResources | Unset = UNSET
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

        missing_resources: dict[str, Any] | Unset = UNSET
        if not isinstance(self.missing_resources, Unset):
            missing_resources = self.missing_resources.to_dict()

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
        if missing_resources is not UNSET:
            field_dict["missing_resources"] = missing_resources

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.missing_setup_resources import (
            MissingSetupResources,
        )

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

        _missing_resources = d.pop("missing_resources", UNSET)
        missing_resources: MissingSetupResources | Unset
        if isinstance(_missing_resources, Unset):
            missing_resources = UNSET
        else:
            missing_resources = MissingSetupResources.from_dict(_missing_resources)

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
            missing_resources=missing_resources,
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
