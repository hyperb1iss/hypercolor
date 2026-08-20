from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

T = TypeVar("T", bound="Rgb")


@_attrs_define
class Rgb:
    """Encoded sRGB color, no alpha. This is what device backends receive.

    Attributes:
        b (int): Blue channel (0–255, sRGB encoded).
        g (int): Green channel (0–255, sRGB encoded).
        r (int): Red channel (0–255, sRGB encoded).
    """

    b: int
    g: int
    r: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        b = self.b

        g = self.g

        r = self.r

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "b": b,
                "g": g,
                "r": r,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        b = d.pop("b")

        g = d.pop("g")

        r = d.pop("r")

        rgb = cls(
            b=b,
            g=g,
            r=r,
        )

        rgb.additional_properties = d
        return rgb

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
