from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.fit_mode import FitMode

if TYPE_CHECKING:
    from ..models.normalized_position import NormalizedPosition


T = TypeVar("T", bound="LayerTransform")


@_attrs_define
class LayerTransform:
    """Geometric placement for a layer source.

    Attributes:
        anchor (NormalizedPosition): A position in normalized `[0.0, 1.0]` canvas space.

            - `(0.0, 0.0)` = top-left corner of the canvas
            - `(1.0, 1.0)` = bottom-right corner of the canvas
            - `(0.5, 0.5)` = center of the canvas

            Values outside `[0.0, 1.0]` are permitted — they represent positions
            beyond the canvas bounds and are handled by [`EdgeBehavior`].

            Used for zone positions and sizes on the canvas, LED positions within
            a zone's bounding box, and space regions in multi-room layouts.
        fit (FitMode): How a cropped viewport maps into a destination canvas.
        rotation (float):
        scale (list[float]):
    """

    anchor: NormalizedPosition
    fit: FitMode
    rotation: float
    scale: list[float]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        anchor = self.anchor.to_dict()

        fit = self.fit.value

        rotation = self.rotation

        scale = self.scale

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "anchor": anchor,
                "fit": fit,
                "rotation": rotation,
                "scale": scale,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.normalized_position import NormalizedPosition

        d = dict(src_dict)
        anchor = NormalizedPosition.from_dict(d.pop("anchor"))

        fit = FitMode(d.pop("fit"))

        rotation = d.pop("rotation")

        scale = cast(list[float], d.pop("scale"))

        layer_transform = cls(
            anchor=anchor,
            fit=fit,
            rotation=rotation,
            scale=scale,
        )

        layer_transform.additional_properties = d
        return layer_transform

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
