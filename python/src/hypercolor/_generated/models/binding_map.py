from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="BindingMap")


@_attrs_define
class BindingMap:
    """Linear mapping from source values into target parameter values.

    Attributes:
        source_max (float):
        source_min (float):
        target_max (float):
        target_min (float):
        clamp (bool | Unset):
    """

    source_max: float
    source_min: float
    target_max: float
    target_min: float
    clamp: bool | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        source_max = self.source_max

        source_min = self.source_min

        target_max = self.target_max

        target_min = self.target_min

        clamp = self.clamp

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "source_max": source_max,
                "source_min": source_min,
                "target_max": target_max,
                "target_min": target_min,
            }
        )
        if clamp is not UNSET:
            field_dict["clamp"] = clamp

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        source_max = d.pop("source_max")

        source_min = d.pop("source_min")

        target_max = d.pop("target_max")

        target_min = d.pop("target_min")

        clamp = d.pop("clamp", UNSET)

        binding_map = cls(
            source_max=source_max,
            source_min=source_min,
            target_max=target_max,
            target_min=target_min,
            clamp=clamp,
        )

        binding_map.additional_properties = d
        return binding_map

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
