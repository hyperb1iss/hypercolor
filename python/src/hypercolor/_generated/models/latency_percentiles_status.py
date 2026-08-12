from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

T = TypeVar("T", bound="LatencyPercentilesStatus")


@_attrs_define
class LatencyPercentilesStatus:
    """
    Attributes:
        avg_ms (float):
        max_ms (float):
        p95_ms (float):
        p99_ms (float):
        sample_count (int):
    """

    avg_ms: float
    max_ms: float
    p95_ms: float
    p99_ms: float
    sample_count: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        avg_ms = self.avg_ms

        max_ms = self.max_ms

        p95_ms = self.p95_ms

        p99_ms = self.p99_ms

        sample_count = self.sample_count

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "avg_ms": avg_ms,
                "max_ms": max_ms,
                "p95_ms": p95_ms,
                "p99_ms": p99_ms,
                "sample_count": sample_count,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        avg_ms = d.pop("avg_ms")

        max_ms = d.pop("max_ms")

        p95_ms = d.pop("p95_ms")

        p99_ms = d.pop("p99_ms")

        sample_count = d.pop("sample_count")

        latency_percentiles_status = cls(
            avg_ms=avg_ms,
            max_ms=max_ms,
            p95_ms=p95_ms,
            p99_ms=p99_ms,
            sample_count=sample_count,
        )

        latency_percentiles_status.additional_properties = d
        return latency_percentiles_status

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
