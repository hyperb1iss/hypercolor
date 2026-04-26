from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.zone_topology_summary_type_0 import ZoneTopologySummaryType0
    from ..models.zone_topology_summary_type_1 import ZoneTopologySummaryType1
    from ..models.zone_topology_summary_type_2 import ZoneTopologySummaryType2
    from ..models.zone_topology_summary_type_3 import ZoneTopologySummaryType3
    from ..models.zone_topology_summary_type_4 import ZoneTopologySummaryType4
    from ..models.zone_topology_summary_type_5 import ZoneTopologySummaryType5


T = TypeVar("T", bound="ZoneSummary")


@_attrs_define
class ZoneSummary:
    """
    Attributes:
        id (str):
        led_count (int):
        name (str):
        topology (str):
        topology_hint (ZoneTopologySummaryType0 | ZoneTopologySummaryType1 | ZoneTopologySummaryType2 |
            ZoneTopologySummaryType3 | ZoneTopologySummaryType4 | ZoneTopologySummaryType5):
    """

    id: str
    led_count: int
    name: str
    topology: str
    topology_hint: (
        ZoneTopologySummaryType0
        | ZoneTopologySummaryType1
        | ZoneTopologySummaryType2
        | ZoneTopologySummaryType3
        | ZoneTopologySummaryType4
        | ZoneTopologySummaryType5
    )
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.zone_topology_summary_type_0 import ZoneTopologySummaryType0
        from ..models.zone_topology_summary_type_1 import ZoneTopologySummaryType1
        from ..models.zone_topology_summary_type_2 import ZoneTopologySummaryType2
        from ..models.zone_topology_summary_type_3 import ZoneTopologySummaryType3
        from ..models.zone_topology_summary_type_4 import ZoneTopologySummaryType4

        id = self.id

        led_count = self.led_count

        name = self.name

        topology = self.topology

        topology_hint: dict[str, Any]
        if isinstance(self.topology_hint, ZoneTopologySummaryType0):
            topology_hint = self.topology_hint.to_dict()
        elif isinstance(self.topology_hint, ZoneTopologySummaryType1):
            topology_hint = self.topology_hint.to_dict()
        elif isinstance(self.topology_hint, ZoneTopologySummaryType2):
            topology_hint = self.topology_hint.to_dict()
        elif isinstance(self.topology_hint, ZoneTopologySummaryType3):
            topology_hint = self.topology_hint.to_dict()
        elif isinstance(self.topology_hint, ZoneTopologySummaryType4):
            topology_hint = self.topology_hint.to_dict()
        else:
            topology_hint = self.topology_hint.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "id": id,
                "led_count": led_count,
                "name": name,
                "topology": topology,
                "topology_hint": topology_hint,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.zone_topology_summary_type_0 import ZoneTopologySummaryType0
        from ..models.zone_topology_summary_type_1 import ZoneTopologySummaryType1
        from ..models.zone_topology_summary_type_2 import ZoneTopologySummaryType2
        from ..models.zone_topology_summary_type_3 import ZoneTopologySummaryType3
        from ..models.zone_topology_summary_type_4 import ZoneTopologySummaryType4
        from ..models.zone_topology_summary_type_5 import ZoneTopologySummaryType5

        d = dict(src_dict)
        id = d.pop("id")

        led_count = d.pop("led_count")

        name = d.pop("name")

        topology = d.pop("topology")

        def _parse_topology_hint(
            data: object,
        ) -> (
            ZoneTopologySummaryType0
            | ZoneTopologySummaryType1
            | ZoneTopologySummaryType2
            | ZoneTopologySummaryType3
            | ZoneTopologySummaryType4
            | ZoneTopologySummaryType5
        ):
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_zone_topology_summary_type_0 = (
                    ZoneTopologySummaryType0.from_dict(data)
                )

                return componentsschemas_zone_topology_summary_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_zone_topology_summary_type_1 = (
                    ZoneTopologySummaryType1.from_dict(data)
                )

                return componentsschemas_zone_topology_summary_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_zone_topology_summary_type_2 = (
                    ZoneTopologySummaryType2.from_dict(data)
                )

                return componentsschemas_zone_topology_summary_type_2
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_zone_topology_summary_type_3 = (
                    ZoneTopologySummaryType3.from_dict(data)
                )

                return componentsschemas_zone_topology_summary_type_3
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_zone_topology_summary_type_4 = (
                    ZoneTopologySummaryType4.from_dict(data)
                )

                return componentsschemas_zone_topology_summary_type_4
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            if not isinstance(data, dict):
                raise TypeError()
            componentsschemas_zone_topology_summary_type_5 = (
                ZoneTopologySummaryType5.from_dict(data)
            )

            return componentsschemas_zone_topology_summary_type_5

        topology_hint = _parse_topology_hint(d.pop("topology_hint"))

        zone_summary = cls(
            id=id,
            led_count=led_count,
            name=name,
            topology=topology,
            topology_hint=topology_hint,
        )

        zone_summary.additional_properties = d
        return zone_summary

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
