from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

T = TypeVar("T", bound="GradientStop")


@_attrs_define
class GradientStop:
    """A single stop in a color gradient.

    Position is normalized `0.0..=1.0` along the gradient axis.
    Color is stored as linear RGBA (`[f32; 4]`).

        Attributes:
            color (list[float]): Linear RGBA color at this stop.
            position (float): Position along the gradient axis, `0.0` = start, `1.0` = end.
    """

    color: list[float]
    position: float
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        color = self.color

        position = self.position

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "color": color,
                "position": position,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        color = cast(list[float], d.pop("color"))

        position = d.pop("position")

        gradient_stop = cls(
            color=color,
            position=position,
        )

        gradient_stop.additional_properties = d
        return gradient_stop

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
