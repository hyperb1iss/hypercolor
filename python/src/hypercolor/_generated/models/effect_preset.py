from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast
from uuid import UUID

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.effect_preset_controls import EffectPresetControls


T = TypeVar("T", bound="EffectPreset")


@_attrs_define
class EffectPreset:
    """A saved parameter snapshot for one effect.

    Attributes:
        effect_id (UUID): Unique identifier for an effect, wrapping a UUID v7.

            Generated at discovery time and used as the primary key across
            the registry, event bus, API, and UI.
        id (UUID): Opaque identifier for an effect preset.
        name (str):
        controls (EffectPresetControls | Unset):
        created_at_ms (int | Unset):
        description (None | str | Unset):
        tags (list[str] | Unset):
        updated_at_ms (int | Unset):
    """

    effect_id: UUID
    id: UUID
    name: str
    controls: EffectPresetControls | Unset = UNSET
    created_at_ms: int | Unset = UNSET
    description: None | str | Unset = UNSET
    tags: list[str] | Unset = UNSET
    updated_at_ms: int | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        effect_id = str(self.effect_id)

        id = str(self.id)

        name = self.name

        controls: dict[str, Any] | Unset = UNSET
        if not isinstance(self.controls, Unset):
            controls = self.controls.to_dict()

        created_at_ms = self.created_at_ms

        description: None | str | Unset
        if isinstance(self.description, Unset):
            description = UNSET
        else:
            description = self.description

        tags: list[str] | Unset = UNSET
        if not isinstance(self.tags, Unset):
            tags = self.tags

        updated_at_ms = self.updated_at_ms

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "effect_id": effect_id,
                "id": id,
                "name": name,
            }
        )
        if controls is not UNSET:
            field_dict["controls"] = controls
        if created_at_ms is not UNSET:
            field_dict["created_at_ms"] = created_at_ms
        if description is not UNSET:
            field_dict["description"] = description
        if tags is not UNSET:
            field_dict["tags"] = tags
        if updated_at_ms is not UNSET:
            field_dict["updated_at_ms"] = updated_at_ms

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.effect_preset_controls import EffectPresetControls

        d = dict(src_dict)
        effect_id = UUID(d.pop("effect_id"))

        id = UUID(d.pop("id"))

        name = d.pop("name")

        _controls = d.pop("controls", UNSET)
        controls: EffectPresetControls | Unset
        if isinstance(_controls, Unset):
            controls = UNSET
        else:
            controls = EffectPresetControls.from_dict(_controls)

        created_at_ms = d.pop("created_at_ms", UNSET)

        def _parse_description(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        description = _parse_description(d.pop("description", UNSET))

        tags = cast(list[str], d.pop("tags", UNSET))

        updated_at_ms = d.pop("updated_at_ms", UNSET)

        effect_preset = cls(
            effect_id=effect_id,
            id=id,
            name=name,
            controls=controls,
            created_at_ms=created_at_ms,
            description=description,
            tags=tags,
            updated_at_ms=updated_at_ms,
        )

        effect_preset.additional_properties = d
        return effect_preset

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
