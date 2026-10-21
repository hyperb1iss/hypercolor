from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.gpu_error_status import GpuErrorStatus
    from ..models.render_failure_status import RenderFailureStatus


T = TypeVar("T", bound="RenderSupervisionStatus")


@_attrs_define
class RenderSupervisionStatus:
    """Health and recovery state of the supervised render thread.

    The render thread restarts its pipeline in-process after a panic or an
    error exit, within a bounded budget, and the daemon exits so its
    service manager restarts it once that budget runs out or the compositor
    GPU device is lost.

        Attributes:
            consecutive_failures (int): Failures since the pipeline last ran past its stable uptime.
            max_restarts (int): Restarts the budget allows within its window.
            phase (str): `not_started`, `starting`, `running`, `restarting`, `terminating`,
                or `stopped`.
            restarts_total (int): In-process pipeline restarts since the daemon started.
            gpu_errors (GpuErrorStatus | Unset): Uncaptured wgpu errors and device losses since the daemon started.
            last_failure (None | RenderFailureStatus | Unset):
            last_frame_age_ms (int | None | Unset): Milliseconds since the render loop last completed a frame.
    """

    consecutive_failures: int
    max_restarts: int
    phase: str
    restarts_total: int
    gpu_errors: GpuErrorStatus | Unset = UNSET
    last_failure: None | RenderFailureStatus | Unset = UNSET
    last_frame_age_ms: int | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.render_failure_status import RenderFailureStatus

        consecutive_failures = self.consecutive_failures

        max_restarts = self.max_restarts

        phase = self.phase

        restarts_total = self.restarts_total

        gpu_errors: dict[str, Any] | Unset = UNSET
        if not isinstance(self.gpu_errors, Unset):
            gpu_errors = self.gpu_errors.to_dict()

        last_failure: dict[str, Any] | None | Unset
        if isinstance(self.last_failure, Unset):
            last_failure = UNSET
        elif isinstance(self.last_failure, RenderFailureStatus):
            last_failure = self.last_failure.to_dict()
        else:
            last_failure = self.last_failure

        last_frame_age_ms: int | None | Unset
        if isinstance(self.last_frame_age_ms, Unset):
            last_frame_age_ms = UNSET
        else:
            last_frame_age_ms = self.last_frame_age_ms

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "consecutive_failures": consecutive_failures,
                "max_restarts": max_restarts,
                "phase": phase,
                "restarts_total": restarts_total,
            }
        )
        if gpu_errors is not UNSET:
            field_dict["gpu_errors"] = gpu_errors
        if last_failure is not UNSET:
            field_dict["last_failure"] = last_failure
        if last_frame_age_ms is not UNSET:
            field_dict["last_frame_age_ms"] = last_frame_age_ms

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.gpu_error_status import GpuErrorStatus
        from ..models.render_failure_status import RenderFailureStatus

        d = dict(src_dict)
        consecutive_failures = d.pop("consecutive_failures")

        max_restarts = d.pop("max_restarts")

        phase = d.pop("phase")

        restarts_total = d.pop("restarts_total")

        _gpu_errors = d.pop("gpu_errors", UNSET)
        gpu_errors: GpuErrorStatus | Unset
        if isinstance(_gpu_errors, Unset):
            gpu_errors = UNSET
        else:
            gpu_errors = GpuErrorStatus.from_dict(_gpu_errors)

        def _parse_last_failure(data: object) -> None | RenderFailureStatus | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                last_failure_type_0 = RenderFailureStatus.from_dict(data)

                return last_failure_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | RenderFailureStatus | Unset, data)

        last_failure = _parse_last_failure(d.pop("last_failure", UNSET))

        def _parse_last_frame_age_ms(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        last_frame_age_ms = _parse_last_frame_age_ms(d.pop("last_frame_age_ms", UNSET))

        render_supervision_status = cls(
            consecutive_failures=consecutive_failures,
            max_restarts=max_restarts,
            phase=phase,
            restarts_total=restarts_total,
            gpu_errors=gpu_errors,
            last_failure=last_failure,
            last_frame_age_ms=last_frame_age_ms,
        )

        render_supervision_status.additional_properties = d
        return render_supervision_status

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
