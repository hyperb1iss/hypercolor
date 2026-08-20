from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.segment_topology_summary_type_0 import SegmentTopologySummaryType0
    from ..models.segment_topology_summary_type_1 import SegmentTopologySummaryType1
    from ..models.segment_topology_summary_type_2 import SegmentTopologySummaryType2
    from ..models.segment_topology_summary_type_3 import SegmentTopologySummaryType3
    from ..models.segment_topology_summary_type_4 import SegmentTopologySummaryType4
    from ..models.segment_topology_summary_type_5 import SegmentTopologySummaryType5


T = TypeVar("T", bound="SegmentSummary")


@_attrs_define
class SegmentSummary:
    """One LED segment of a device (hardware topology, not scene render zones).

    Attributes:
        id (str):
        led_count (int):
        name (str):
        topology (str):
        topology_hint (None | SegmentTopologySummaryType0 | SegmentTopologySummaryType1 | SegmentTopologySummaryType2 |
            SegmentTopologySummaryType3 | SegmentTopologySummaryType4 | SegmentTopologySummaryType5 | Unset):
    """

    id: str
    led_count: int
    name: str
    topology: str
    topology_hint: (
        None
        | SegmentTopologySummaryType0
        | SegmentTopologySummaryType1
        | SegmentTopologySummaryType2
        | SegmentTopologySummaryType3
        | SegmentTopologySummaryType4
        | SegmentTopologySummaryType5
        | Unset
    ) = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.segment_topology_summary_type_0 import SegmentTopologySummaryType0
        from ..models.segment_topology_summary_type_1 import SegmentTopologySummaryType1
        from ..models.segment_topology_summary_type_2 import SegmentTopologySummaryType2
        from ..models.segment_topology_summary_type_3 import SegmentTopologySummaryType3
        from ..models.segment_topology_summary_type_4 import SegmentTopologySummaryType4
        from ..models.segment_topology_summary_type_5 import SegmentTopologySummaryType5

        id = self.id

        led_count = self.led_count

        name = self.name

        topology = self.topology

        topology_hint: dict[str, Any] | None | Unset
        if isinstance(self.topology_hint, Unset):
            topology_hint = UNSET
        elif isinstance(self.topology_hint, SegmentTopologySummaryType0):
            topology_hint = self.topology_hint.to_dict()
        elif isinstance(self.topology_hint, SegmentTopologySummaryType1):
            topology_hint = self.topology_hint.to_dict()
        elif isinstance(self.topology_hint, SegmentTopologySummaryType2):
            topology_hint = self.topology_hint.to_dict()
        elif isinstance(self.topology_hint, SegmentTopologySummaryType3):
            topology_hint = self.topology_hint.to_dict()
        elif isinstance(self.topology_hint, SegmentTopologySummaryType4):
            topology_hint = self.topology_hint.to_dict()
        elif isinstance(self.topology_hint, SegmentTopologySummaryType5):
            topology_hint = self.topology_hint.to_dict()
        else:
            topology_hint = self.topology_hint

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "id": id,
                "led_count": led_count,
                "name": name,
                "topology": topology,
            }
        )
        if topology_hint is not UNSET:
            field_dict["topology_hint"] = topology_hint

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.segment_topology_summary_type_0 import SegmentTopologySummaryType0
        from ..models.segment_topology_summary_type_1 import SegmentTopologySummaryType1
        from ..models.segment_topology_summary_type_2 import SegmentTopologySummaryType2
        from ..models.segment_topology_summary_type_3 import SegmentTopologySummaryType3
        from ..models.segment_topology_summary_type_4 import SegmentTopologySummaryType4
        from ..models.segment_topology_summary_type_5 import SegmentTopologySummaryType5

        d = dict(src_dict)
        id = d.pop("id")

        led_count = d.pop("led_count")

        name = d.pop("name")

        topology = d.pop("topology")

        def _parse_topology_hint(
            data: object,
        ) -> (
            None
            | SegmentTopologySummaryType0
            | SegmentTopologySummaryType1
            | SegmentTopologySummaryType2
            | SegmentTopologySummaryType3
            | SegmentTopologySummaryType4
            | SegmentTopologySummaryType5
            | Unset
        ):
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_segment_topology_summary_type_0 = (
                    SegmentTopologySummaryType0.from_dict(data)
                )

                return componentsschemas_segment_topology_summary_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_segment_topology_summary_type_1 = (
                    SegmentTopologySummaryType1.from_dict(data)
                )

                return componentsschemas_segment_topology_summary_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_segment_topology_summary_type_2 = (
                    SegmentTopologySummaryType2.from_dict(data)
                )

                return componentsschemas_segment_topology_summary_type_2
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_segment_topology_summary_type_3 = (
                    SegmentTopologySummaryType3.from_dict(data)
                )

                return componentsschemas_segment_topology_summary_type_3
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_segment_topology_summary_type_4 = (
                    SegmentTopologySummaryType4.from_dict(data)
                )

                return componentsschemas_segment_topology_summary_type_4
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_segment_topology_summary_type_5 = (
                    SegmentTopologySummaryType5.from_dict(data)
                )

                return componentsschemas_segment_topology_summary_type_5
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(
                None
                | SegmentTopologySummaryType0
                | SegmentTopologySummaryType1
                | SegmentTopologySummaryType2
                | SegmentTopologySummaryType3
                | SegmentTopologySummaryType4
                | SegmentTopologySummaryType5
                | Unset,
                data,
            )

        topology_hint = _parse_topology_hint(d.pop("topology_hint", UNSET))

        segment_summary = cls(
            id=id,
            led_count=led_count,
            name=name,
            topology=topology,
            topology_hint=topology_hint,
        )

        segment_summary.additional_properties = d
        return segment_summary

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
