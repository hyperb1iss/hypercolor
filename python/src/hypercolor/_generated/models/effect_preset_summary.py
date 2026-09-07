from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.effect_preset_origin import EffectPresetOrigin
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.effect_preset_summary_controls import EffectPresetSummaryControls


T = TypeVar("T", bound="EffectPresetSummary")


@_attrs_define
class EffectPresetSummary:
    """One bundled or saved preset projected through an effect-scoped API.

    Attributes:
        controls (EffectPresetSummaryControls):
        editable (bool):
        effect_id (str):
        id (str):
        name (str):
        origin (EffectPresetOrigin): Origin of a preset in an effect's unified preset stack.
        description (None | str | Unset):
        tags (list[str] | Unset):
    """

    controls: EffectPresetSummaryControls
    editable: bool
    effect_id: str
    id: str
    name: str
    origin: EffectPresetOrigin
    description: None | str | Unset = UNSET
    tags: list[str] | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        controls = self.controls.to_dict()

        editable = self.editable

        effect_id = self.effect_id

        id = self.id

        name = self.name

        origin = self.origin.value

        description: None | str | Unset
        if isinstance(self.description, Unset):
            description = UNSET
        else:
            description = self.description

        tags: list[str] | Unset = UNSET
        if not isinstance(self.tags, Unset):
            tags = self.tags

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "controls": controls,
                "editable": editable,
                "effect_id": effect_id,
                "id": id,
                "name": name,
                "origin": origin,
            }
        )
        if description is not UNSET:
            field_dict["description"] = description
        if tags is not UNSET:
            field_dict["tags"] = tags

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.effect_preset_summary_controls import EffectPresetSummaryControls

        d = dict(src_dict)
        controls = EffectPresetSummaryControls.from_dict(d.pop("controls"))

        editable = d.pop("editable")

        effect_id = d.pop("effect_id")

        id = d.pop("id")

        name = d.pop("name")

        origin = EffectPresetOrigin(d.pop("origin"))

        def _parse_description(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        description = _parse_description(d.pop("description", UNSET))

        tags = cast(list[str], d.pop("tags", UNSET))

        effect_preset_summary = cls(
            controls=controls,
            editable=editable,
            effect_id=effect_id,
            id=id,
            name=name,
            origin=origin,
            description=description,
            tags=tags,
        )

        effect_preset_summary.additional_properties = d
        return effect_preset_summary

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
