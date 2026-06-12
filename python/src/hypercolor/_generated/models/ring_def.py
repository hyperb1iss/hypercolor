from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.winding import Winding

T = TypeVar("T", bound="RingDef")


@_attrs_define
class RingDef:
    """Definition for a single ring within [`LedTopology::ConcentricRings`].

    Attributes:
        count (int): Number of LEDs in this ring.
        direction (Winding): Winding direction for circular topologies.
        radius (float): Radius as a fraction of the zone's half-size. 0.0 = center, 1.0 = zone edge.
        start_angle (float): Angle of LED 0 in radians.
    """

    count: int
    direction: Winding
    radius: float
    start_angle: float
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        count = self.count

        direction = self.direction.value

        radius = self.radius

        start_angle = self.start_angle

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "count": count,
                "direction": direction,
                "radius": radius,
                "start_angle": start_angle,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        count = d.pop("count")

        direction = Winding(d.pop("direction"))

        radius = d.pop("radius")

        start_angle = d.pop("start_angle")

        ring_def = cls(
            count=count,
            direction=direction,
            radius=radius,
            start_angle=start_angle,
        )

        ring_def.additional_properties = d
        return ring_def

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
