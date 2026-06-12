from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.sampling_mode_type_2_type import SamplingModeType2Type

T = TypeVar("T", bound="SamplingModeType2")


@_attrs_define
class SamplingModeType2:
    """Flat average of a rectangular region. O(1) with summed-area table.

    Attributes:
        radius_x (float): Half-width of the averaging rectangle in pixels.
        radius_y (float): Half-height of the averaging rectangle in pixels.
        type_ (SamplingModeType2Type):
    """

    radius_x: float
    radius_y: float
    type_: SamplingModeType2Type
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        radius_x = self.radius_x

        radius_y = self.radius_y

        type_ = self.type_.value

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "radius_x": radius_x,
                "radius_y": radius_y,
                "type": type_,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        radius_x = d.pop("radius_x")

        radius_y = d.pop("radius_y")

        type_ = SamplingModeType2Type(d.pop("type"))

        sampling_mode_type_2 = cls(
            radius_x=radius_x,
            radius_y=radius_y,
            type_=type_,
        )

        sampling_mode_type_2.additional_properties = d
        return sampling_mode_type_2

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
