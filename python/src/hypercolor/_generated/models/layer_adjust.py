from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

T = TypeVar("T", bound="LayerAdjust")


@_attrs_define
class LayerAdjust:
    """Per-layer color adjustment settings.

    Attributes:
        brightness (float):
        contrast (float):
        hue_shift (float):
        saturation (float):
        tint (list[float]):
        tint_strength (float):
    """

    brightness: float
    contrast: float
    hue_shift: float
    saturation: float
    tint: list[float]
    tint_strength: float
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        brightness = self.brightness

        contrast = self.contrast

        hue_shift = self.hue_shift

        saturation = self.saturation

        tint = self.tint

        tint_strength = self.tint_strength

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "brightness": brightness,
                "contrast": contrast,
                "hue_shift": hue_shift,
                "saturation": saturation,
                "tint": tint,
                "tint_strength": tint_strength,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        brightness = d.pop("brightness")

        contrast = d.pop("contrast")

        hue_shift = d.pop("hue_shift")

        saturation = d.pop("saturation")

        tint = cast(list[float], d.pop("tint"))

        tint_strength = d.pop("tint_strength")

        layer_adjust = cls(
            brightness=brightness,
            contrast=contrast,
            hue_shift=hue_shift,
            saturation=saturation,
            tint=tint,
            tint_strength=tint_strength,
        )

        layer_adjust.additional_properties = d
        return layer_adjust

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
