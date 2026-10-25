from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.gpu_pass_timing_status import GpuPassTimingStatus


T = TypeVar("T", bound="GpuPassTimingsStatus")


@_attrs_define
class GpuPassTimingsStatus:
    """GPU timestamp coverage for SparkleFlinger's compute passes.

    Durations exclude Servo's separate GPU context, uploads, transfer commands,
    and queue latency. Counters belong to the current compositor instance.

        Attributes:
            enabled (bool):
            stages (list[GpuPassTimingStatus]):
            supported (bool):
            instance_id (int | Unset): Changes when the compositor is recreated; zero means no compositor.
            unavailable_reason (None | str | Unset):
    """

    enabled: bool
    stages: list[GpuPassTimingStatus]
    supported: bool
    instance_id: int | Unset = UNSET
    unavailable_reason: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        enabled = self.enabled

        stages = []
        for stages_item_data in self.stages:
            stages_item = stages_item_data.to_dict()
            stages.append(stages_item)

        supported = self.supported

        instance_id = self.instance_id

        unavailable_reason: None | str | Unset
        if isinstance(self.unavailable_reason, Unset):
            unavailable_reason = UNSET
        else:
            unavailable_reason = self.unavailable_reason

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "enabled": enabled,
                "stages": stages,
                "supported": supported,
            }
        )
        if instance_id is not UNSET:
            field_dict["instance_id"] = instance_id
        if unavailable_reason is not UNSET:
            field_dict["unavailable_reason"] = unavailable_reason

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.gpu_pass_timing_status import GpuPassTimingStatus

        d = dict(src_dict)
        enabled = d.pop("enabled")

        stages = []
        _stages = d.pop("stages")
        for stages_item_data in _stages:
            stages_item = GpuPassTimingStatus.from_dict(stages_item_data)

            stages.append(stages_item)

        supported = d.pop("supported")

        instance_id = d.pop("instance_id", UNSET)

        def _parse_unavailable_reason(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        unavailable_reason = _parse_unavailable_reason(
            d.pop("unavailable_reason", UNSET)
        )

        gpu_pass_timings_status = cls(
            enabled=enabled,
            stages=stages,
            supported=supported,
            instance_id=instance_id,
            unavailable_reason=unavailable_reason,
        )

        gpu_pass_timings_status.additional_properties = d
        return gpu_pass_timings_status

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
