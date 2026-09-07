from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.orientation import Orientation
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


T = TypeVar("T", bound="MemberPlacement")


@_attrs_define
class MemberPlacement:
    """One member's spatial placement inside its zone's layout.

    Deliberately tolerant of unknown fields: this shape is dual-use
    (request body and zone-resource read-back), and the response-side
    client-tolerance convention wins for embedded resources. The
    strict envelope around it still rejects unknown top-level fields.

        Attributes:
            member (str): A zone membership's identity — wire-transparent, unique within its
                zone, which is all its zone-scoped route needs.
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
            topology (LedTopologyType0 | LedTopologyType1 | LedTopologyType2 | LedTopologyType3 | LedTopologyType4 |
                LedTopologyType5 | LedTopologyType6): LED arrangement within a zone's bounding rectangle.

                Each variant computes zone-local positions in normalized `[0.0, 1.0]` space.
                The topology determines how many LEDs exist and where they sit within
                the zone's rectangular bounds.
            orientation (None | Orientation | Unset):
            rotation (float | Unset):
            scale (float | Unset):
    """

    member: str
    position: NormalizedPosition
    size: NormalizedPosition
    topology: (
        LedTopologyType0
        | LedTopologyType1
        | LedTopologyType2
        | LedTopologyType3
        | LedTopologyType4
        | LedTopologyType5
        | LedTopologyType6
    )
    orientation: None | Orientation | Unset = UNSET
    rotation: float | Unset = UNSET
    scale: float | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.led_topology_type_0 import LedTopologyType0
        from ..models.led_topology_type_1 import LedTopologyType1
        from ..models.led_topology_type_2 import LedTopologyType2
        from ..models.led_topology_type_3 import LedTopologyType3
        from ..models.led_topology_type_4 import LedTopologyType4
        from ..models.led_topology_type_5 import LedTopologyType5

        member = self.member

        position = self.position.to_dict()

        size = self.size.to_dict()

        topology: dict[str, Any]
        if isinstance(self.topology, LedTopologyType0):
            topology = self.topology.to_dict()
        elif isinstance(self.topology, LedTopologyType1):
            topology = self.topology.to_dict()
        elif isinstance(self.topology, LedTopologyType2):
            topology = self.topology.to_dict()
        elif isinstance(self.topology, LedTopologyType3):
            topology = self.topology.to_dict()
        elif isinstance(self.topology, LedTopologyType4):
            topology = self.topology.to_dict()
        elif isinstance(self.topology, LedTopologyType5):
            topology = self.topology.to_dict()
        else:
            topology = self.topology.to_dict()

        orientation: None | str | Unset
        if isinstance(self.orientation, Unset):
            orientation = UNSET
        elif isinstance(self.orientation, Orientation):
            orientation = self.orientation.value
        else:
            orientation = self.orientation

        rotation = self.rotation

        scale = self.scale

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "member": member,
                "position": position,
                "size": size,
                "topology": topology,
            }
        )
        if orientation is not UNSET:
            field_dict["orientation"] = orientation
        if rotation is not UNSET:
            field_dict["rotation"] = rotation
        if scale is not UNSET:
            field_dict["scale"] = scale

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.led_topology_type_0 import LedTopologyType0
        from ..models.led_topology_type_1 import LedTopologyType1
        from ..models.led_topology_type_2 import LedTopologyType2
        from ..models.led_topology_type_3 import LedTopologyType3
        from ..models.led_topology_type_4 import LedTopologyType4
        from ..models.led_topology_type_5 import LedTopologyType5
        from ..models.led_topology_type_6 import LedTopologyType6
        from ..models.normalized_position import NormalizedPosition

        d = dict(src_dict)
        member = d.pop("member")

        position = NormalizedPosition.from_dict(d.pop("position"))

        size = NormalizedPosition.from_dict(d.pop("size"))

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
        ):
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
            if not isinstance(data, dict):
                raise TypeError()
            componentsschemas_led_topology_type_6 = LedTopologyType6.from_dict(data)

            return componentsschemas_led_topology_type_6

        topology = _parse_topology(d.pop("topology"))

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

        member_placement = cls(
            member=member,
            position=position,
            size=size,
            topology=topology,
            orientation=orientation,
            rotation=rotation,
            scale=scale,
        )

        member_placement.additional_properties = d
        return member_placement

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
