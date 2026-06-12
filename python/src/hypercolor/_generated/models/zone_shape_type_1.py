from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.zone_shape_type_1_shape_type import ZoneShapeType1ShapeType

T = TypeVar("T", bound="ZoneShapeType1")


@_attrs_define
class ZoneShapeType1:
    """Circular arc or full circle (fans).

    Attributes:
        shape_type (ZoneShapeType1ShapeType):
        start_angle (float): Start angle in radians.
        sweep_angle (float): Sweep angle in radians.
    """

    shape_type: ZoneShapeType1ShapeType
    start_angle: float
    sweep_angle: float
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        shape_type = self.shape_type.value

        start_angle = self.start_angle

        sweep_angle = self.sweep_angle

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "shape_type": shape_type,
                "start_angle": start_angle,
                "sweep_angle": sweep_angle,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        shape_type = ZoneShapeType1ShapeType(d.pop("shape_type"))

        start_angle = d.pop("start_angle")

        sweep_angle = d.pop("sweep_angle")

        zone_shape_type_1 = cls(
            shape_type=shape_type,
            start_angle=start_angle,
            sweep_angle=sweep_angle,
        )

        zone_shape_type_1.additional_properties = d
        return zone_shape_type_1

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
