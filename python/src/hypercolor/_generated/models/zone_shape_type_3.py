from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.zone_shape_type_3_shape_type import ZoneShapeType3ShapeType

if TYPE_CHECKING:
    from ..models.normalized_position import NormalizedPosition


T = TypeVar("T", bound="ZoneShapeType3")


@_attrs_define
class ZoneShapeType3:
    """Arbitrary polygon defined by normalized vertices.

    Attributes:
        shape_type (ZoneShapeType3ShapeType):
        vertices (list[NormalizedPosition]): Polygon vertices in normalized coordinates.
    """

    shape_type: ZoneShapeType3ShapeType
    vertices: list[NormalizedPosition]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        shape_type = self.shape_type.value

        vertices = []
        for vertices_item_data in self.vertices:
            vertices_item = vertices_item_data.to_dict()
            vertices.append(vertices_item)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "shape_type": shape_type,
                "vertices": vertices,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.normalized_position import NormalizedPosition

        d = dict(src_dict)
        shape_type = ZoneShapeType3ShapeType(d.pop("shape_type"))

        vertices = []
        _vertices = d.pop("vertices")
        for vertices_item_data in _vertices:
            vertices_item = NormalizedPosition.from_dict(vertices_item_data)

            vertices.append(vertices_item)

        zone_shape_type_3 = cls(
            shape_type=shape_type,
            vertices=vertices,
        )

        zone_shape_type_3.additional_properties = d
        return zone_shape_type_3

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
