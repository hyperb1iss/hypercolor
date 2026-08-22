from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.b_tree_map_additional_property_type_9_kind import (
    BTreeMapAdditionalPropertyType9Kind,
)

if TYPE_CHECKING:
    from ..models.rgb import Rgb


T = TypeVar("T", bound="BTreeMapAdditionalPropertyType9")


@_attrs_define
class BTreeMapAdditionalPropertyType9:
    """
    Attributes:
        kind (BTreeMapAdditionalPropertyType9Kind):
        value (Rgb): Encoded sRGB color, no alpha. This is what device backends receive.
    """

    kind: BTreeMapAdditionalPropertyType9Kind
    value: Rgb
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        kind = self.kind.value

        value = self.value.to_dict()

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
        from ..models.rgb import Rgb

        d = dict(src_dict)
        kind = BTreeMapAdditionalPropertyType9Kind(d.pop("kind"))

        value = Rgb.from_dict(d.pop("value"))

        b_tree_map_additional_property_type_9 = cls(
            kind=kind,
            value=value,
        )

        b_tree_map_additional_property_type_9.additional_properties = d
        return b_tree_map_additional_property_type_9

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
