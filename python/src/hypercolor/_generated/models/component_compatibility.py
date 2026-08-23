from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="ComponentCompatibility")


@_attrs_define
class ComponentCompatibility:
    """Controller/slot matcher for a reusable template.

    Empty matcher fields are wildcards. If a template has no compatibility
    entries at all, it is considered globally compatible.

        Attributes:
            controller_ids (list[str] | Unset): Controller driver or protocol identifiers.
            models (list[str] | Unset): Optional model identifiers, such as `prism_s`.
            slots (list[str] | Unset): Optional slot identifiers, such as `gpu`.
    """

    controller_ids: list[str] | Unset = UNSET
    models: list[str] | Unset = UNSET
    slots: list[str] | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        controller_ids: list[str] | Unset = UNSET
        if not isinstance(self.controller_ids, Unset):
            controller_ids = self.controller_ids

        models: list[str] | Unset = UNSET
        if not isinstance(self.models, Unset):
            models = self.models

        slots: list[str] | Unset = UNSET
        if not isinstance(self.slots, Unset):
            slots = self.slots

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if controller_ids is not UNSET:
            field_dict["controller_ids"] = controller_ids
        if models is not UNSET:
            field_dict["models"] = models
        if slots is not UNSET:
            field_dict["slots"] = slots

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        controller_ids = cast(list[str], d.pop("controller_ids", UNSET))

        models = cast(list[str], d.pop("models", UNSET))

        slots = cast(list[str], d.pop("slots", UNSET))

        component_compatibility = cls(
            controller_ids=controller_ids,
            models=models,
            slots=slots,
        )

        component_compatibility.additional_properties = d
        return component_compatibility

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
