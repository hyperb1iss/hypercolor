from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.b_tree_map_additional_property_type_0_kind import (
    BTreeMapAdditionalPropertyType0Kind,
)

T = TypeVar("T", bound="BTreeMapAdditionalPropertyType0")


@_attrs_define
class BTreeMapAdditionalPropertyType0:
    """
    Attributes:
        kind (BTreeMapAdditionalPropertyType0Kind):
    """

    kind: BTreeMapAdditionalPropertyType0Kind
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        kind = self.kind.value

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "kind": kind,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        kind = BTreeMapAdditionalPropertyType0Kind(d.pop("kind"))

        b_tree_map_additional_property_type_0 = cls(
            kind=kind,
        )

        b_tree_map_additional_property_type_0.additional_properties = d
        return b_tree_map_additional_property_type_0

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
