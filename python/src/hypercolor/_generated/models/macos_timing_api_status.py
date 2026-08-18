from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

T = TypeVar("T", bound="MacosTimingApiStatus")


@_attrs_define
class MacosTimingApiStatus:
    """
    Attributes:
        max_ns (int):
        p95_ns (int):
        p99_ns (int):
        sample_count (int):
        total_ns (int):
    """

    max_ns: int
    p95_ns: int
    p99_ns: int
    sample_count: int
    total_ns: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        max_ns = self.max_ns

        p95_ns = self.p95_ns

        p99_ns = self.p99_ns

        sample_count = self.sample_count

        total_ns = self.total_ns

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "max_ns": max_ns,
                "p95_ns": p95_ns,
                "p99_ns": p99_ns,
                "sample_count": sample_count,
                "total_ns": total_ns,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        max_ns = d.pop("max_ns")

        p95_ns = d.pop("p95_ns")

        p99_ns = d.pop("p99_ns")

        sample_count = d.pop("sample_count")

        total_ns = d.pop("total_ns")

        macos_timing_api_status = cls(
            max_ns=max_ns,
            p95_ns=p95_ns,
            p99_ns=p99_ns,
            sample_count=sample_count,
            total_ns=total_ns,
        )

        macos_timing_api_status.additional_properties = d
        return macos_timing_api_status

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
