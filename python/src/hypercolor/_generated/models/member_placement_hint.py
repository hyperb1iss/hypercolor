from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define

from ..models.orientation import Orientation
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.normalized_position import NormalizedPosition


T = TypeVar("T", bound="MemberPlacementHint")


@_attrs_define
class MemberPlacementHint:
    """Placement-only seed; device binding and topology remain daemon-owned.

    Attributes:
        position (NormalizedPosition): A position in normalized `[0.0, 1.0]` canvas space.

            - `(0.0, 0.0)` = top-left corner of the canvas
            - `(1.0, 1.0)` = bottom-right corner of the canvas
            - `(0.5, 0.5)` = center of the canvas

            Values outside `[0.0, 1.0]` are permitted — they represent positions
            beyond the canvas bounds and are handled by [`EdgeBehavior`].

            Used for zone positions and sizes on the canvas, LED positions within
            a zone's bounding box, and space regions in multi-room layouts.
        size (NormalizedPosition): A position in normalized `[0.0, 1.0]` canvas space.

            - `(0.0, 0.0)` = top-left corner of the canvas
            - `(1.0, 1.0)` = bottom-right corner of the canvas
            - `(0.5, 0.5)` = center of the canvas

            Values outside `[0.0, 1.0]` are permitted — they represent positions
            beyond the canvas bounds and are handled by [`EdgeBehavior`].

            Used for zone positions and sizes on the canvas, LED positions within
            a zone's bounding box, and space regions in multi-room layouts.
        orientation (None | Orientation | Unset):
        rotation (float | Unset):
        scale (float | Unset):
        segment (None | str | Unset):
    """

    position: NormalizedPosition
    size: NormalizedPosition
    orientation: None | Orientation | Unset = UNSET
    rotation: float | Unset = UNSET
    scale: float | Unset = UNSET
    segment: None | str | Unset = UNSET

    def to_dict(self) -> dict[str, Any]:
        position = self.position.to_dict()

        size = self.size.to_dict()

        orientation: None | str | Unset
        if isinstance(self.orientation, Unset):
            orientation = UNSET
        elif isinstance(self.orientation, Orientation):
            orientation = self.orientation.value
        else:
            orientation = self.orientation

        rotation = self.rotation

        scale = self.scale

        segment: None | str | Unset
        if isinstance(self.segment, Unset):
            segment = UNSET
        else:
            segment = self.segment

        field_dict: dict[str, Any] = {}

        field_dict.update(
            {
                "position": position,
                "size": size,
            }
        )
        if orientation is not UNSET:
            field_dict["orientation"] = orientation
        if rotation is not UNSET:
            field_dict["rotation"] = rotation
        if scale is not UNSET:
            field_dict["scale"] = scale
        if segment is not UNSET:
            field_dict["segment"] = segment

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.normalized_position import NormalizedPosition

        d = dict(src_dict)
        position = NormalizedPosition.from_dict(d.pop("position"))

        size = NormalizedPosition.from_dict(d.pop("size"))

        def _parse_orientation(data: object) -> None | Orientation | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                orientation_type_1 = Orientation(data)

                return orientation_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | Orientation | Unset, data)

        orientation = _parse_orientation(d.pop("orientation", UNSET))

        rotation = d.pop("rotation", UNSET)

        scale = d.pop("scale", UNSET)

        def _parse_segment(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        segment = _parse_segment(d.pop("segment", UNSET))

        member_placement_hint = cls(
            position=position,
            size=size,
            orientation=orientation,
            rotation=rotation,
            scale=scale,
            segment=segment,
        )

        return member_placement_hint
