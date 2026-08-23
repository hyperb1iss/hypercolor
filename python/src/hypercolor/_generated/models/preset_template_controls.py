from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.control_value import ControlValue


T = TypeVar("T", bound="PresetTemplateControls")


@_attrs_define
class PresetTemplateControls:
    """Control values that define this preset. Keys are control IDs."""

    additional_properties: dict[str, ControlValue] = _attrs_field(
        init=False, factory=dict
    )

    def to_dict(self) -> dict[str, Any]:

        field_dict: dict[str, Any] = {}
        for prop_name, prop in self.additional_properties.items():
            field_dict[prop_name] = prop.to_dict()

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.control_value import ControlValue

        d = dict(src_dict)
        preset_template_controls = cls()

        additional_properties = {}
        for prop_name, prop_dict in d.items():
            additional_property = ControlValue.from_dict(prop_dict)

            additional_properties[prop_name] = additional_property

        preset_template_controls.additional_properties = additional_properties
        return preset_template_controls

    @property
    def additional_keys(self) -> list[str]:
        return list(self.additional_properties.keys())

    def __getitem__(self, key: str) -> ControlValue:
        return self.additional_properties[key]

    def __setitem__(self, key: str, value: ControlValue) -> None:
        self.additional_properties[key] = value

    def __delitem__(self, key: str) -> None:
        del self.additional_properties[key]

    def __contains__(self, key: str) -> bool:
        return key in self.additional_properties
