from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

if TYPE_CHECKING:
    from ..models.normalized_position import NormalizedPosition


T = TypeVar("T", bound="SegmentPlacement")


@_attrs_define
class SegmentPlacement:
    """A segment's bounding box in the device's physical coordinate system.

    Coordinates use one isotropic unit across every sibling, with x pointing
    right and y pointing down. The position is the box center, not necessarily
    the lighting centroid; custom topology carries the actual sample position.

        Attributes:
            position (NormalizedPosition): A position in normalized `[0.0, 1.0]` canvas space.

                - `(0.0, 0.0)` = top-left corner of the canvas
                - `(1.0, 1.0)` = bottom-right corner of the canvas
                - `(0.5, 0.5)` = center of the canvas

                Values outside `[0.0, 1.0]` are permitted — they represent positions
                beyond the canvas bounds and are handled by [`EdgeBehavior`].

                Used for zone positions and sizes on the canvas, LED positions within
                a zone's bounding box, and space regions in multi-room layouts.
            rotation (float): Rotation in radians about the bounding-box center, matching [`crate::spatial::Output`].
            size (NormalizedPosition): A position in normalized `[0.0, 1.0]` canvas space.

                - `(0.0, 0.0)` = top-left corner of the canvas
                - `(1.0, 1.0)` = bottom-right corner of the canvas
                - `(0.5, 0.5)` = center of the canvas

                Values outside `[0.0, 1.0]` are permitted — they represent positions
                beyond the canvas bounds and are handled by [`EdgeBehavior`].

                Used for zone positions and sizes on the canvas, LED positions within
                a zone's bounding box, and space regions in multi-room layouts.
    """

    position: NormalizedPosition
    rotation: float
    size: NormalizedPosition
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        position = self.position.to_dict()

        rotation = self.rotation

        size = self.size.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "position": position,
                "rotation": rotation,
                "size": size,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.normalized_position import NormalizedPosition

        d = dict(src_dict)
        position = NormalizedPosition.from_dict(d.pop("position"))

        rotation = d.pop("rotation")

        size = NormalizedPosition.from_dict(d.pop("size"))

        segment_placement = cls(
            position=position,
            rotation=rotation,
            size=size,
        )

        segment_placement.additional_properties = d
        return segment_placement

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
