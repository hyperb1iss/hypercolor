from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.control_value import ControlValue


T = TypeVar("T", bound="ApplyEffectRequestControlsType0")


@_attrs_define
class ApplyEffectRequestControlsType0:
    """ """

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
        apply_effect_request_controls_type_0 = cls()

        additional_properties = {}
        for prop_name, prop_dict in d.items():
            additional_property = ControlValue.from_dict(prop_dict)

            additional_properties[prop_name] = additional_property

        apply_effect_request_controls_type_0.additional_properties = (
            additional_properties
        )
        return apply_effect_request_controls_type_0

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
