from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.b_tree_map_additional_property_type_11_kind import (
    BTreeMapAdditionalPropertyType11Kind,
)

if TYPE_CHECKING:
    from ..models.linear_rgba import LinearRgba


T = TypeVar("T", bound="BTreeMapAdditionalPropertyType11")


@_attrs_define
class BTreeMapAdditionalPropertyType11:
    """
    Attributes:
        kind (BTreeMapAdditionalPropertyType11Kind):
        value (LinearRgba): Linear-light RGBA color with straight alpha, `0.0..=1.0` per channel.

            All interpolation, blending, and perceptual conversion happens here.
            Out-of-range values are legal mid-pipeline (HDR headroom, out-of-gamut
            Oklab results) and clamp on conversion back to bytes.
    """

    kind: BTreeMapAdditionalPropertyType11Kind
    value: LinearRgba
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
        from ..models.linear_rgba import LinearRgba

        d = dict(src_dict)
        kind = BTreeMapAdditionalPropertyType11Kind(d.pop("kind"))

        value = LinearRgba.from_dict(d.pop("value"))

        b_tree_map_additional_property_type_11 = cls(
            kind=kind,
            value=value,
        )

        b_tree_map_additional_property_type_11.additional_properties = d
        return b_tree_map_additional_property_type_11

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
