from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.full_frame_copy_session_status import FullFrameCopySessionStatus
    from ..models.latency_percentiles_status import LatencyPercentilesStatus


T = TypeVar("T", bound="SessionPerformanceStatus")


@_attrs_define
class SessionPerformanceStatus:
    """
    Attributes:
        full_frame_cpu_copies (FullFrameCopySessionStatus):
        input_stage (LatencyPercentilesStatus):
    """

    full_frame_cpu_copies: FullFrameCopySessionStatus
    input_stage: LatencyPercentilesStatus
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        full_frame_cpu_copies = self.full_frame_cpu_copies.to_dict()

        input_stage = self.input_stage.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "full_frame_cpu_copies": full_frame_cpu_copies,
                "input_stage": input_stage,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.full_frame_copy_session_status import FullFrameCopySessionStatus
        from ..models.latency_percentiles_status import LatencyPercentilesStatus

        d = dict(src_dict)
        full_frame_cpu_copies = FullFrameCopySessionStatus.from_dict(
            d.pop("full_frame_cpu_copies")
        )

        input_stage = LatencyPercentilesStatus.from_dict(d.pop("input_stage"))

        session_performance_status = cls(
            full_frame_cpu_copies=full_frame_cpu_copies,
            input_stage=input_stage,
        )

        session_performance_status.additional_properties = d
        return session_performance_status

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
