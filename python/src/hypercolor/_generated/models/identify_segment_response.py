from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="IdentifySegmentResponse")


@_attrs_define
class IdentifySegmentResponse:
    """Response for `POST /api/v1/devices/{id}/segments/{segment}/identify`.

    Attributes:
        device_id (str):
        duration_ms (int):
        identifying (bool):
        segment (str):
        segment_name (str):
        color (None | str | Unset):
    """

    device_id: str
    duration_ms: int
    identifying: bool
    segment: str
    segment_name: str
    color: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        device_id = self.device_id

        duration_ms = self.duration_ms

        identifying = self.identifying

        segment = self.segment

        segment_name = self.segment_name

        color: None | str | Unset
        if isinstance(self.color, Unset):
            color = UNSET
        else:
            color = self.color

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "device_id": device_id,
                "duration_ms": duration_ms,
                "identifying": identifying,
                "segment": segment,
                "segment_name": segment_name,
            }
        )
        if color is not UNSET:
            field_dict["color"] = color

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        device_id = d.pop("device_id")

        duration_ms = d.pop("duration_ms")

        identifying = d.pop("identifying")

        segment = d.pop("segment")

        segment_name = d.pop("segment_name")

        def _parse_color(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        color = _parse_color(d.pop("color", UNSET))

        identify_segment_response = cls(
            device_id=device_id,
            duration_ms=duration_ms,
            identifying=identifying,
            segment=segment,
            segment_name=segment_name,
            color=color,
        )

        identify_segment_response.additional_properties = d
        return identify_segment_response

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
