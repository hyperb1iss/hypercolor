from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

T = TypeVar("T", bound="LinearRgba")


@_attrs_define
class LinearRgba:
    """Linear-light RGBA color with straight alpha, `0.0..=1.0` per channel.

    All interpolation, blending, and perceptual conversion happens here.
    Out-of-range values are legal mid-pipeline (HDR headroom, out-of-gamut
    Oklab results) and clamp on conversion back to bytes.

        Attributes:
            a (float): Alpha (0.0 = transparent, 1.0 = opaque; never gamma encoded).
            b (float): Blue channel (linear light).
            g (float): Green channel (linear light).
            r (float): Red channel (linear light).
    """

    a: float
    b: float
    g: float
    r: float
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        a = self.a

        b = self.b

        g = self.g

        r = self.r

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "a": a,
                "b": b,
                "g": g,
                "r": r,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        a = d.pop("a")

        b = d.pop("b")

        g = d.pop("g")

        r = d.pop("r")

        linear_rgba = cls(
            a=a,
            b=b,
            g=g,
            r=r,
        )

        linear_rgba.additional_properties = d
        return linear_rgba

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
