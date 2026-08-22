from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.b_tree_map_additional_property_type_12_kind import (
    BTreeMapAdditionalPropertyType12Kind,
)

if TYPE_CHECKING:
    from ..models.gradient_stop import GradientStop


T = TypeVar("T", bound="BTreeMapAdditionalPropertyType12")


@_attrs_define
class BTreeMapAdditionalPropertyType12:
    """
    Attributes:
        kind (BTreeMapAdditionalPropertyType12Kind):
        value (list[GradientStop]):
    """

    kind: BTreeMapAdditionalPropertyType12Kind
    value: list[GradientStop]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        kind = self.kind.value

        value = []
        for value_item_data in self.value:
            value_item = value_item_data.to_dict()
            value.append(value_item)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "kind": kind,
                "value": value,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.gradient_stop import GradientStop

        d = dict(src_dict)
        kind = BTreeMapAdditionalPropertyType12Kind(d.pop("kind"))

        value = []
        _value = d.pop("value")
        for value_item_data in _value:
            value_item = GradientStop.from_dict(value_item_data)

            value.append(value_item)

        b_tree_map_additional_property_type_12 = cls(
            kind=kind,
            value=value,
        )

        b_tree_map_additional_property_type_12.additional_properties = d
        return b_tree_map_additional_property_type_12

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
