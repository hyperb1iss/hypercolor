from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.led_topology_type_0 import LedTopologyType0
    from ..models.led_topology_type_1 import LedTopologyType1
    from ..models.led_topology_type_2 import LedTopologyType2
    from ..models.led_topology_type_3 import LedTopologyType3
    from ..models.led_topology_type_4 import LedTopologyType4
    from ..models.led_topology_type_5 import LedTopologyType5
    from ..models.led_topology_type_6 import LedTopologyType6
    from ..models.normalized_position import NormalizedPosition
    from ..models.segment_placement import SegmentPlacement
    from ..models.zone_shape_type_0 import ZoneShapeType0
    from ..models.zone_shape_type_1 import ZoneShapeType1
    from ..models.zone_shape_type_2 import ZoneShapeType2
    from ..models.zone_shape_type_3 import ZoneShapeType3


T = TypeVar("T", bound="SegmentLayoutHint")


@_attrs_define
class SegmentLayoutHint:
    """Device-declared spatial hint used when auto-generating or repairing layouts.

    Attributes:
        co_located (bool | Unset): Place this segment at the same center as sibling segments.
        placement (None | SegmentPlacement | Unset):
        shape (None | Unset | ZoneShapeType0 | ZoneShapeType1 | ZoneShapeType2 | ZoneShapeType3):
        size (None | NormalizedPosition | Unset):
        topology (LedTopologyType0 | LedTopologyType1 | LedTopologyType2 | LedTopologyType3 | LedTopologyType4 |
            LedTopologyType5 | LedTopologyType6 | None | Unset):
    """

    co_located: bool | Unset = UNSET
    placement: None | SegmentPlacement | Unset = UNSET
    shape: (
        None | Unset | ZoneShapeType0 | ZoneShapeType1 | ZoneShapeType2 | ZoneShapeType3
    ) = UNSET
    size: None | NormalizedPosition | Unset = UNSET
    topology: (
        LedTopologyType0
        | LedTopologyType1
        | LedTopologyType2
        | LedTopologyType3
        | LedTopologyType4
        | LedTopologyType5
        | LedTopologyType6
        | None
        | Unset
    ) = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.led_topology_type_0 import LedTopologyType0
        from ..models.led_topology_type_1 import LedTopologyType1
        from ..models.led_topology_type_2 import LedTopologyType2
        from ..models.led_topology_type_3 import LedTopologyType3
        from ..models.led_topology_type_4 import LedTopologyType4
        from ..models.led_topology_type_5 import LedTopologyType5
        from ..models.led_topology_type_6 import LedTopologyType6
        from ..models.normalized_position import NormalizedPosition
        from ..models.segment_placement import SegmentPlacement
        from ..models.zone_shape_type_0 import ZoneShapeType0
        from ..models.zone_shape_type_1 import ZoneShapeType1
        from ..models.zone_shape_type_2 import ZoneShapeType2
        from ..models.zone_shape_type_3 import ZoneShapeType3

        co_located = self.co_located

        placement: dict[str, Any] | None | Unset
        if isinstance(self.placement, Unset):
            placement = UNSET
        elif isinstance(self.placement, SegmentPlacement):
            placement = self.placement.to_dict()
        else:
            placement = self.placement

        shape: dict[str, Any] | None | Unset
        if isinstance(self.shape, Unset):
            shape = UNSET
        elif (
            isinstance(self.shape, ZoneShapeType0)
            or isinstance(self.shape, ZoneShapeType1)
            or isinstance(self.shape, ZoneShapeType2)
            or isinstance(self.shape, ZoneShapeType3)
        ):
            shape = self.shape.to_dict()
        else:
            shape = self.shape

        size: dict[str, Any] | None | Unset
        if isinstance(self.size, Unset):
            size = UNSET
        elif isinstance(self.size, NormalizedPosition):
            size = self.size.to_dict()
        else:
            size = self.size

        topology: dict[str, Any] | None | Unset
        if isinstance(self.topology, Unset):
            topology = UNSET
        elif (
            isinstance(self.topology, LedTopologyType0)
            or isinstance(self.topology, LedTopologyType1)
            or isinstance(self.topology, LedTopologyType2)
            or isinstance(self.topology, LedTopologyType3)
            or isinstance(self.topology, LedTopologyType4)
            or isinstance(self.topology, LedTopologyType5)
            or isinstance(self.topology, LedTopologyType6)
        ):
            topology = self.topology.to_dict()
        else:
            topology = self.topology

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if co_located is not UNSET:
            field_dict["co_located"] = co_located
        if placement is not UNSET:
            field_dict["placement"] = placement
        if shape is not UNSET:
            field_dict["shape"] = shape
        if size is not UNSET:
            field_dict["size"] = size
        if topology is not UNSET:
            field_dict["topology"] = topology

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.led_topology_type_0 import LedTopologyType0
        from ..models.led_topology_type_1 import LedTopologyType1
        from ..models.led_topology_type_2 import LedTopologyType2
        from ..models.led_topology_type_3 import LedTopologyType3
        from ..models.led_topology_type_4 import LedTopologyType4
        from ..models.led_topology_type_5 import LedTopologyType5
        from ..models.led_topology_type_6 import LedTopologyType6
        from ..models.normalized_position import NormalizedPosition
        from ..models.segment_placement import SegmentPlacement
        from ..models.zone_shape_type_0 import ZoneShapeType0
        from ..models.zone_shape_type_1 import ZoneShapeType1
        from ..models.zone_shape_type_2 import ZoneShapeType2
        from ..models.zone_shape_type_3 import ZoneShapeType3

        d = dict(src_dict)
        co_located = d.pop("co_located", UNSET)

        def _parse_placement(data: object) -> None | SegmentPlacement | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                placement_type_0 = SegmentPlacement.from_dict(data)

                return placement_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | SegmentPlacement | Unset, data)

        placement = _parse_placement(d.pop("placement", UNSET))

        def _parse_shape(
            data: object,
        ) -> (
            None
            | Unset
            | ZoneShapeType0
            | ZoneShapeType1
            | ZoneShapeType2
            | ZoneShapeType3
        ):
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_zone_shape_type_0 = ZoneShapeType0.from_dict(data)

                return componentsschemas_zone_shape_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_zone_shape_type_1 = ZoneShapeType1.from_dict(data)

                return componentsschemas_zone_shape_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_zone_shape_type_2 = ZoneShapeType2.from_dict(data)

                return componentsschemas_zone_shape_type_2
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_zone_shape_type_3 = ZoneShapeType3.from_dict(data)

                return componentsschemas_zone_shape_type_3
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(
                None
                | Unset
                | ZoneShapeType0
                | ZoneShapeType1
                | ZoneShapeType2
                | ZoneShapeType3,
                data,
            )

        shape = _parse_shape(d.pop("shape", UNSET))

        def _parse_size(data: object) -> None | NormalizedPosition | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                size_type_0 = NormalizedPosition.from_dict(data)

                return size_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | NormalizedPosition | Unset, data)

        size = _parse_size(d.pop("size", UNSET))

        def _parse_topology(
            data: object,
        ) -> (
            LedTopologyType0
            | LedTopologyType1
            | LedTopologyType2
            | LedTopologyType3
            | LedTopologyType4
            | LedTopologyType5
            | LedTopologyType6
            | None
            | Unset
        ):
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_led_topology_type_0 = LedTopologyType0.from_dict(data)

                return componentsschemas_led_topology_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_led_topology_type_1 = LedTopologyType1.from_dict(data)

                return componentsschemas_led_topology_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_led_topology_type_2 = LedTopologyType2.from_dict(data)

                return componentsschemas_led_topology_type_2
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_led_topology_type_3 = LedTopologyType3.from_dict(data)

                return componentsschemas_led_topology_type_3
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_led_topology_type_4 = LedTopologyType4.from_dict(data)

                return componentsschemas_led_topology_type_4
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_led_topology_type_5 = LedTopologyType5.from_dict(data)

                return componentsschemas_led_topology_type_5
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_led_topology_type_6 = LedTopologyType6.from_dict(data)

                return componentsschemas_led_topology_type_6
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(
                LedTopologyType0
                | LedTopologyType1
                | LedTopologyType2
                | LedTopologyType3
                | LedTopologyType4
                | LedTopologyType5
                | LedTopologyType6
                | None
                | Unset,
                data,
            )

        topology = _parse_topology(d.pop("topology", UNSET))

        segment_layout_hint = cls(
            co_located=co_located,
            placement=placement,
            shape=shape,
            size=size,
            topology=topology,
        )

        segment_layout_hint.additional_properties = d
        return segment_layout_hint

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
