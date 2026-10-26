from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

T = TypeVar("T", bound="GpuPassTimingStatus")


@_attrs_define
class GpuPassTimingStatus:
    """Cumulative GPU execution time for one class of compute passes.

    Pending passes are attempts minus completed, skipped, failed, and cancelled
    passes. No duration is fabricated for passes without a completed timestamp.

        Attributes:
            attempts (int):
            cancelled (int):
            completed (int):
            failed (int):
            last_ns (int):
            max_ns (int):
            skipped (int):
            stage (str):
            total_ns (int):
    """

    attempts: int
    cancelled: int
    completed: int
    failed: int
    last_ns: int
    max_ns: int
    skipped: int
    stage: str
    total_ns: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        attempts = self.attempts

        cancelled = self.cancelled

        completed = self.completed

        failed = self.failed

        last_ns = self.last_ns

        max_ns = self.max_ns

        skipped = self.skipped

        stage = self.stage

        total_ns = self.total_ns

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "attempts": attempts,
                "cancelled": cancelled,
                "completed": completed,
                "failed": failed,
                "last_ns": last_ns,
                "max_ns": max_ns,
                "skipped": skipped,
                "stage": stage,
                "total_ns": total_ns,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        attempts = d.pop("attempts")

        cancelled = d.pop("cancelled")

        completed = d.pop("completed")

        failed = d.pop("failed")

        last_ns = d.pop("last_ns")

        max_ns = d.pop("max_ns")

        skipped = d.pop("skipped")

        stage = d.pop("stage")

        total_ns = d.pop("total_ns")

        gpu_pass_timing_status = cls(
            attempts=attempts,
            cancelled=cancelled,
            completed=completed,
            failed=failed,
            last_ns=last_ns,
            max_ns=max_ns,
            skipped=skipped,
            stage=stage,
            total_ns=total_ns,
        )

        gpu_pass_timing_status.additional_properties = d
        return gpu_pass_timing_status

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
