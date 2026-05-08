from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.gpu_compositor_probe_status import GpuCompositorProbeStatus


T = TypeVar("T", bound="RenderAccelerationStatus")


@_attrs_define
class RenderAccelerationStatus:
    """
    Attributes:
        effective_mode (str):
        requested_mode (str):
        servo_gpu_import_attempting (bool):
        servo_gpu_import_mode (str):
        fallback_reason (None | str | Unset):
        gpu_probe (GpuCompositorProbeStatus | None | Unset):
    """

    effective_mode: str
    requested_mode: str
    servo_gpu_import_attempting: bool
    servo_gpu_import_mode: str
    fallback_reason: None | str | Unset = UNSET
    gpu_probe: GpuCompositorProbeStatus | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.gpu_compositor_probe_status import GpuCompositorProbeStatus

        effective_mode = self.effective_mode

        requested_mode = self.requested_mode

        servo_gpu_import_attempting = self.servo_gpu_import_attempting

        servo_gpu_import_mode = self.servo_gpu_import_mode

        fallback_reason: None | str | Unset
        if isinstance(self.fallback_reason, Unset):
            fallback_reason = UNSET
        else:
            fallback_reason = self.fallback_reason

        gpu_probe: dict[str, Any] | None | Unset
        if isinstance(self.gpu_probe, Unset):
            gpu_probe = UNSET
        elif isinstance(self.gpu_probe, GpuCompositorProbeStatus):
            gpu_probe = self.gpu_probe.to_dict()
        else:
            gpu_probe = self.gpu_probe

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "effective_mode": effective_mode,
                "requested_mode": requested_mode,
                "servo_gpu_import_attempting": servo_gpu_import_attempting,
                "servo_gpu_import_mode": servo_gpu_import_mode,
            }
        )
        if fallback_reason is not UNSET:
            field_dict["fallback_reason"] = fallback_reason
        if gpu_probe is not UNSET:
            field_dict["gpu_probe"] = gpu_probe

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.gpu_compositor_probe_status import GpuCompositorProbeStatus

        d = dict(src_dict)
        effective_mode = d.pop("effective_mode")

        requested_mode = d.pop("requested_mode")

        servo_gpu_import_attempting = d.pop("servo_gpu_import_attempting")

        servo_gpu_import_mode = d.pop("servo_gpu_import_mode")

        def _parse_fallback_reason(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        fallback_reason = _parse_fallback_reason(d.pop("fallback_reason", UNSET))

        def _parse_gpu_probe(data: object) -> GpuCompositorProbeStatus | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                gpu_probe_type_1 = GpuCompositorProbeStatus.from_dict(data)

                return gpu_probe_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(GpuCompositorProbeStatus | None | Unset, data)

        gpu_probe = _parse_gpu_probe(d.pop("gpu_probe", UNSET))

        render_acceleration_status = cls(
            effective_mode=effective_mode,
            requested_mode=requested_mode,
            servo_gpu_import_attempting=servo_gpu_import_attempting,
            servo_gpu_import_mode=servo_gpu_import_mode,
            fallback_reason=fallback_reason,
            gpu_probe=gpu_probe,
        )

        render_acceleration_status.additional_properties = d
        return render_acceleration_status

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
