from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

T = TypeVar("T", bound="NormalizedPosition")


@_attrs_define
class NormalizedPosition:
    """A position in normalized `[0.0, 1.0]` canvas space.

    - `(0.0, 0.0)` = top-left corner of the canvas
    - `(1.0, 1.0)` = bottom-right corner of the canvas
    - `(0.5, 0.5)` = center of the canvas

    Values outside `[0.0, 1.0]` are permitted — they represent positions
    beyond the canvas bounds and are handled by [`EdgeBehavior`].

    Used for zone positions and sizes on the canvas, LED positions within
    a zone's bounding box, and space regions in multi-room layouts.

        Attributes:
            x (float): Horizontal position. 0.0 = left edge, 1.0 = right edge.
            y (float): Vertical position. 0.0 = top edge, 1.0 = bottom edge.
    """

    x: float
    y: float
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        x = self.x

        y = self.y

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "x": x,
                "y": y,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        x = d.pop("x")

        y = d.pop("y")

        normalized_position = cls(
            x=x,
            y=y,
        )

        normalized_position.additional_properties = d
        return normalized_position

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
