from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="FavoriteSummary")


@_attrs_define
class FavoriteSummary:
    """One favorited effect.

    `effect_name` is resolved from the registry at request time and falls
    back to the id when the effect is no longer installed.

        Attributes:
            effect_id (str):
            added_at_ms (int | Unset):
            effect_name (str | Unset):
    """

    effect_id: str
    added_at_ms: int | Unset = UNSET
    effect_name: str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        effect_id = self.effect_id

        added_at_ms = self.added_at_ms

        effect_name = self.effect_name

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "effect_id": effect_id,
            }
        )
        if added_at_ms is not UNSET:
            field_dict["added_at_ms"] = added_at_ms
        if effect_name is not UNSET:
            field_dict["effect_name"] = effect_name

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        effect_id = d.pop("effect_id")

        added_at_ms = d.pop("added_at_ms", UNSET)

        effect_name = d.pop("effect_name", UNSET)

        favorite_summary = cls(
            effect_id=effect_id,
            added_at_ms=added_at_ms,
            effect_name=effect_name,
        )

        favorite_summary.additional_properties = d
        return favorite_summary

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
