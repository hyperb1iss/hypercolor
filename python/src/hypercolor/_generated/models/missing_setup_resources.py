from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar
from uuid import UUID

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..types import UNSET, Unset

T = TypeVar("T", bound="MissingSetupResources")


@_attrs_define
class MissingSetupResources:
    """Referenced content that must be installed or imported on this computer.

    Attributes:
        effects (list[UUID] | Unset):
        media (list[UUID] | Unset):
        presets (list[UUID] | Unset):
    """

    effects: list[UUID] | Unset = UNSET
    media: list[UUID] | Unset = UNSET
    presets: list[UUID] | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        effects: list[str] | Unset = UNSET
        if not isinstance(self.effects, Unset):
            effects = []
            for effects_item_data in self.effects:
                effects_item = str(effects_item_data)
                effects.append(effects_item)

        media: list[str] | Unset = UNSET
        if not isinstance(self.media, Unset):
            media = []
            for media_item_data in self.media:
                media_item = str(media_item_data)
                media.append(media_item)

        presets: list[str] | Unset = UNSET
        if not isinstance(self.presets, Unset):
            presets = []
            for presets_item_data in self.presets:
                presets_item = str(presets_item_data)
                presets.append(presets_item)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if effects is not UNSET:
            field_dict["effects"] = effects
        if media is not UNSET:
            field_dict["media"] = media
        if presets is not UNSET:
            field_dict["presets"] = presets

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        _effects = d.pop("effects", UNSET)
        effects: list[UUID] | Unset = UNSET
        if _effects is not UNSET:
            effects = []
            for effects_item_data in _effects:
                effects_item = UUID(effects_item_data)

                effects.append(effects_item)

        _media = d.pop("media", UNSET)
        media: list[UUID] | Unset = UNSET
        if _media is not UNSET:
            media = []
            for media_item_data in _media:
                media_item = UUID(media_item_data)

                media.append(media_item)

        _presets = d.pop("presets", UNSET)
        presets: list[UUID] | Unset = UNSET
        if _presets is not UNSET:
            presets = []
            for presets_item_data in _presets:
                presets_item = UUID(presets_item_data)

                presets.append(presets_item)

        missing_setup_resources = cls(
            effects=effects,
            media=media,
            presets=presets,
        )

        missing_setup_resources.additional_properties = d
        return missing_setup_resources

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
