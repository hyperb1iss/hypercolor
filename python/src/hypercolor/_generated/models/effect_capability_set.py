from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="EffectCapabilitySet")


@_attrs_define
class EffectCapabilitySet:
    """Typed source requirements declared by an effect.

    Attributes:
        audio_reactive (bool | Unset):
        input_reactive (bool | Unset):
        screen_reactive (bool | Unset):
    """

    audio_reactive: bool | Unset = UNSET
    input_reactive: bool | Unset = UNSET
    screen_reactive: bool | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        audio_reactive = self.audio_reactive

        input_reactive = self.input_reactive

        screen_reactive = self.screen_reactive

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if audio_reactive is not UNSET:
            field_dict["audio_reactive"] = audio_reactive
        if input_reactive is not UNSET:
            field_dict["input_reactive"] = input_reactive
        if screen_reactive is not UNSET:
            field_dict["screen_reactive"] = screen_reactive

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        audio_reactive = d.pop("audio_reactive", UNSET)

        input_reactive = d.pop("input_reactive", UNSET)

        screen_reactive = d.pop("screen_reactive", UNSET)

        effect_capability_set = cls(
            audio_reactive=audio_reactive,
            input_reactive=input_reactive,
            screen_reactive=screen_reactive,
        )

        effect_capability_set.additional_properties = d
        return effect_capability_set

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
