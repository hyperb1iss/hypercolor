from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.sampling_mode_type_3_type import SamplingModeType3Type

T = TypeVar("T", bound="SamplingModeType3")


@_attrs_define
class SamplingModeType3:
    """Gaussian-weighted average for natural ambient falloff.

    Attributes:
        radius (int): Kernel half-size in pixels (full kernel = `(2*radius+1)^2`).
        sigma (float): Standard deviation of the Gaussian kernel.
        type_ (SamplingModeType3Type):
    """

    radius: int
    sigma: float
    type_: SamplingModeType3Type
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        radius = self.radius

        sigma = self.sigma

        type_ = self.type_.value

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "radius": radius,
                "sigma": sigma,
                "type": type_,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        radius = d.pop("radius")

        sigma = d.pop("sigma")

        type_ = SamplingModeType3Type(d.pop("type"))

        sampling_mode_type_3 = cls(
            radius=radius,
            sigma=sigma,
            type_=type_,
        )

        sampling_mode_type_3.additional_properties = d
        return sampling_mode_type_3

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
