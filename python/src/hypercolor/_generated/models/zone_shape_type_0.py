from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.zone_shape_type_0_shape_type import ZoneShapeType0ShapeType

T = TypeVar("T", bound="ZoneShapeType0")


@_attrs_define
class ZoneShapeType0:
    """Rectangular bounding box (default for strips, matrices).

    Attributes:
        shape_type (ZoneShapeType0ShapeType):
    """

    shape_type: ZoneShapeType0ShapeType
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        shape_type = self.shape_type.value

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "shape_type": shape_type,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        shape_type = ZoneShapeType0ShapeType(d.pop("shape_type"))

        zone_shape_type_0 = cls(
            shape_type=shape_type,
        )

        zone_shape_type_0.additional_properties = d
        return zone_shape_type_0

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
