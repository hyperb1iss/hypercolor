from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.preset_template_controls import PresetTemplateControls


T = TypeVar("T", bound="PresetTemplate")


@_attrs_define
class PresetTemplate:
    """An effect-defined preset — a named snapshot of control values bundled
    with the effect itself. Unlike user-created [`super::library::EffectPreset`]s,
    these are authored by the effect developer and are read-only at runtime.

        Attributes:
            name (str): Human-readable preset name (e.g. "Sunset Glow", "Deep Ocean").
            controls (PresetTemplateControls | Unset): Control values that define this preset. Keys are control IDs.
            description (None | str | Unset): Optional short description.
    """

    name: str
    controls: PresetTemplateControls | Unset = UNSET
    description: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        name = self.name

        controls: dict[str, Any] | Unset = UNSET
        if not isinstance(self.controls, Unset):
            controls = self.controls.to_dict()

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
        if controls is not UNSET:
            field_dict["controls"] = controls
        if description is not UNSET:
            field_dict["description"] = description

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.preset_template_controls import PresetTemplateControls

        d = dict(src_dict)
        name = d.pop("name")

        _controls = d.pop("controls", UNSET)
        controls: PresetTemplateControls | Unset
        if isinstance(_controls, Unset):
            controls = UNSET
        else:
            controls = PresetTemplateControls.from_dict(_controls)

        def _parse_description(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        description = _parse_description(d.pop("description", UNSET))

        preset_template = cls(
            name=name,
            controls=controls,
            description=description,
        )

        preset_template.additional_properties = d
        return preset_template

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
