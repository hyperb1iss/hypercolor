from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="EffectHealthStatus")


@_attrs_define
class EffectHealthStatus:
    """
    Attributes:
        errors_total (int):
        fallbacks_applied_total (int):
        producer_cpu_frames_total (int):
        producer_gpu_frames_total (int):
        servo_breaker_opens_total (int):
        servo_detached_destroy_failures_total (int):
        servo_detached_destroys_total (int):
        servo_gpu_import_blit_max_ms (float):
        servo_gpu_import_blit_total_ms (float):
        servo_gpu_import_failures_total (int):
        servo_gpu_import_fallbacks_total (int):
        servo_gpu_import_max_ms (float):
        servo_gpu_import_sync_max_ms (float):
        servo_gpu_import_sync_total_ms (float):
        servo_gpu_import_total_ms (float):
        servo_page_load_failures_total (int):
        servo_page_load_wait_max_ms (float):
        servo_page_load_wait_total_ms (float):
        servo_page_loads_total (int):
        servo_render_cached_frames_total (int):
        servo_render_cpu_frames_total (int):
        servo_render_evaluate_scripts_max_ms (float):
        servo_render_evaluate_scripts_total_ms (float):
        servo_render_event_loop_max_ms (float):
        servo_render_event_loop_total_ms (float):
        servo_render_frame_max_ms (float):
        servo_render_frame_total_ms (float):
        servo_render_gpu_frames_total (int):
        servo_render_paint_max_ms (float):
        servo_render_paint_total_ms (float):
        servo_render_queue_wait_max_ms (float):
        servo_render_queue_wait_total_ms (float):
        servo_render_readback_max_ms (float):
        servo_render_readback_total_ms (float):
        servo_render_requests_total (int):
        servo_session_create_failures_total (int):
        servo_session_create_wait_max_ms (float):
        servo_session_create_wait_total_ms (float):
        servo_session_creates_total (int):
        servo_soft_stalls_total (int):
        sparkleflinger_gpu_source_upload_skipped_total (int):
        servo_gpu_import_fallback_reason (None | str | Unset):
    """

    errors_total: int
    fallbacks_applied_total: int
    producer_cpu_frames_total: int
    producer_gpu_frames_total: int
    servo_breaker_opens_total: int
    servo_detached_destroy_failures_total: int
    servo_detached_destroys_total: int
    servo_gpu_import_blit_max_ms: float
    servo_gpu_import_blit_total_ms: float
    servo_gpu_import_failures_total: int
    servo_gpu_import_fallbacks_total: int
    servo_gpu_import_max_ms: float
    servo_gpu_import_sync_max_ms: float
    servo_gpu_import_sync_total_ms: float
    servo_gpu_import_total_ms: float
    servo_page_load_failures_total: int
    servo_page_load_wait_max_ms: float
    servo_page_load_wait_total_ms: float
    servo_page_loads_total: int
    servo_render_cached_frames_total: int
    servo_render_cpu_frames_total: int
    servo_render_evaluate_scripts_max_ms: float
    servo_render_evaluate_scripts_total_ms: float
    servo_render_event_loop_max_ms: float
    servo_render_event_loop_total_ms: float
    servo_render_frame_max_ms: float
    servo_render_frame_total_ms: float
    servo_render_gpu_frames_total: int
    servo_render_paint_max_ms: float
    servo_render_paint_total_ms: float
    servo_render_queue_wait_max_ms: float
    servo_render_queue_wait_total_ms: float
    servo_render_readback_max_ms: float
    servo_render_readback_total_ms: float
    servo_render_requests_total: int
    servo_session_create_failures_total: int
    servo_session_create_wait_max_ms: float
    servo_session_create_wait_total_ms: float
    servo_session_creates_total: int
    servo_soft_stalls_total: int
    sparkleflinger_gpu_source_upload_skipped_total: int
    servo_gpu_import_fallback_reason: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        errors_total = self.errors_total

        fallbacks_applied_total = self.fallbacks_applied_total

        producer_cpu_frames_total = self.producer_cpu_frames_total

        producer_gpu_frames_total = self.producer_gpu_frames_total

        servo_breaker_opens_total = self.servo_breaker_opens_total

        servo_detached_destroy_failures_total = (
            self.servo_detached_destroy_failures_total
        )

        servo_detached_destroys_total = self.servo_detached_destroys_total

        servo_gpu_import_blit_max_ms = self.servo_gpu_import_blit_max_ms

        servo_gpu_import_blit_total_ms = self.servo_gpu_import_blit_total_ms

        servo_gpu_import_failures_total = self.servo_gpu_import_failures_total

        servo_gpu_import_fallbacks_total = self.servo_gpu_import_fallbacks_total

        servo_gpu_import_max_ms = self.servo_gpu_import_max_ms

        servo_gpu_import_sync_max_ms = self.servo_gpu_import_sync_max_ms

        servo_gpu_import_sync_total_ms = self.servo_gpu_import_sync_total_ms

        servo_gpu_import_total_ms = self.servo_gpu_import_total_ms

        servo_page_load_failures_total = self.servo_page_load_failures_total

        servo_page_load_wait_max_ms = self.servo_page_load_wait_max_ms

        servo_page_load_wait_total_ms = self.servo_page_load_wait_total_ms

        servo_page_loads_total = self.servo_page_loads_total

        servo_render_cached_frames_total = self.servo_render_cached_frames_total

        servo_render_cpu_frames_total = self.servo_render_cpu_frames_total

        servo_render_evaluate_scripts_max_ms = self.servo_render_evaluate_scripts_max_ms

        servo_render_evaluate_scripts_total_ms = (
            self.servo_render_evaluate_scripts_total_ms
        )

        servo_render_event_loop_max_ms = self.servo_render_event_loop_max_ms

        servo_render_event_loop_total_ms = self.servo_render_event_loop_total_ms

        servo_render_frame_max_ms = self.servo_render_frame_max_ms

        servo_render_frame_total_ms = self.servo_render_frame_total_ms

        servo_render_gpu_frames_total = self.servo_render_gpu_frames_total

        servo_render_paint_max_ms = self.servo_render_paint_max_ms

        servo_render_paint_total_ms = self.servo_render_paint_total_ms

        servo_render_queue_wait_max_ms = self.servo_render_queue_wait_max_ms

        servo_render_queue_wait_total_ms = self.servo_render_queue_wait_total_ms

        servo_render_readback_max_ms = self.servo_render_readback_max_ms

        servo_render_readback_total_ms = self.servo_render_readback_total_ms

        servo_render_requests_total = self.servo_render_requests_total

        servo_session_create_failures_total = self.servo_session_create_failures_total

        servo_session_create_wait_max_ms = self.servo_session_create_wait_max_ms

        servo_session_create_wait_total_ms = self.servo_session_create_wait_total_ms

        servo_session_creates_total = self.servo_session_creates_total

        servo_soft_stalls_total = self.servo_soft_stalls_total

        sparkleflinger_gpu_source_upload_skipped_total = (
            self.sparkleflinger_gpu_source_upload_skipped_total
        )

        servo_gpu_import_fallback_reason: None | str | Unset
        if isinstance(self.servo_gpu_import_fallback_reason, Unset):
            servo_gpu_import_fallback_reason = UNSET
        else:
            servo_gpu_import_fallback_reason = self.servo_gpu_import_fallback_reason

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "errors_total": errors_total,
                "fallbacks_applied_total": fallbacks_applied_total,
                "producer_cpu_frames_total": producer_cpu_frames_total,
                "producer_gpu_frames_total": producer_gpu_frames_total,
                "servo_breaker_opens_total": servo_breaker_opens_total,
                "servo_detached_destroy_failures_total": servo_detached_destroy_failures_total,
                "servo_detached_destroys_total": servo_detached_destroys_total,
                "servo_gpu_import_blit_max_ms": servo_gpu_import_blit_max_ms,
                "servo_gpu_import_blit_total_ms": servo_gpu_import_blit_total_ms,
                "servo_gpu_import_failures_total": servo_gpu_import_failures_total,
                "servo_gpu_import_fallbacks_total": servo_gpu_import_fallbacks_total,
                "servo_gpu_import_max_ms": servo_gpu_import_max_ms,
                "servo_gpu_import_sync_max_ms": servo_gpu_import_sync_max_ms,
                "servo_gpu_import_sync_total_ms": servo_gpu_import_sync_total_ms,
                "servo_gpu_import_total_ms": servo_gpu_import_total_ms,
                "servo_page_load_failures_total": servo_page_load_failures_total,
                "servo_page_load_wait_max_ms": servo_page_load_wait_max_ms,
                "servo_page_load_wait_total_ms": servo_page_load_wait_total_ms,
                "servo_page_loads_total": servo_page_loads_total,
                "servo_render_cached_frames_total": servo_render_cached_frames_total,
                "servo_render_cpu_frames_total": servo_render_cpu_frames_total,
                "servo_render_evaluate_scripts_max_ms": servo_render_evaluate_scripts_max_ms,
                "servo_render_evaluate_scripts_total_ms": servo_render_evaluate_scripts_total_ms,
                "servo_render_event_loop_max_ms": servo_render_event_loop_max_ms,
                "servo_render_event_loop_total_ms": servo_render_event_loop_total_ms,
                "servo_render_frame_max_ms": servo_render_frame_max_ms,
                "servo_render_frame_total_ms": servo_render_frame_total_ms,
                "servo_render_gpu_frames_total": servo_render_gpu_frames_total,
                "servo_render_paint_max_ms": servo_render_paint_max_ms,
                "servo_render_paint_total_ms": servo_render_paint_total_ms,
                "servo_render_queue_wait_max_ms": servo_render_queue_wait_max_ms,
                "servo_render_queue_wait_total_ms": servo_render_queue_wait_total_ms,
                "servo_render_readback_max_ms": servo_render_readback_max_ms,
                "servo_render_readback_total_ms": servo_render_readback_total_ms,
                "servo_render_requests_total": servo_render_requests_total,
                "servo_session_create_failures_total": servo_session_create_failures_total,
                "servo_session_create_wait_max_ms": servo_session_create_wait_max_ms,
                "servo_session_create_wait_total_ms": servo_session_create_wait_total_ms,
                "servo_session_creates_total": servo_session_creates_total,
                "servo_soft_stalls_total": servo_soft_stalls_total,
                "sparkleflinger_gpu_source_upload_skipped_total": sparkleflinger_gpu_source_upload_skipped_total,
            }
        )
        if servo_gpu_import_fallback_reason is not UNSET:
            field_dict["servo_gpu_import_fallback_reason"] = (
                servo_gpu_import_fallback_reason
            )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        errors_total = d.pop("errors_total")

        fallbacks_applied_total = d.pop("fallbacks_applied_total")

        producer_cpu_frames_total = d.pop("producer_cpu_frames_total")

        producer_gpu_frames_total = d.pop("producer_gpu_frames_total")

        servo_breaker_opens_total = d.pop("servo_breaker_opens_total")

        servo_detached_destroy_failures_total = d.pop(
            "servo_detached_destroy_failures_total"
        )

        servo_detached_destroys_total = d.pop("servo_detached_destroys_total")

        servo_gpu_import_blit_max_ms = d.pop("servo_gpu_import_blit_max_ms")

        servo_gpu_import_blit_total_ms = d.pop("servo_gpu_import_blit_total_ms")

        servo_gpu_import_failures_total = d.pop("servo_gpu_import_failures_total")

        servo_gpu_import_fallbacks_total = d.pop("servo_gpu_import_fallbacks_total")

        servo_gpu_import_max_ms = d.pop("servo_gpu_import_max_ms")

        servo_gpu_import_sync_max_ms = d.pop("servo_gpu_import_sync_max_ms")

        servo_gpu_import_sync_total_ms = d.pop("servo_gpu_import_sync_total_ms")

        servo_gpu_import_total_ms = d.pop("servo_gpu_import_total_ms")

        servo_page_load_failures_total = d.pop("servo_page_load_failures_total")

        servo_page_load_wait_max_ms = d.pop("servo_page_load_wait_max_ms")

        servo_page_load_wait_total_ms = d.pop("servo_page_load_wait_total_ms")

        servo_page_loads_total = d.pop("servo_page_loads_total")

        servo_render_cached_frames_total = d.pop("servo_render_cached_frames_total")

        servo_render_cpu_frames_total = d.pop("servo_render_cpu_frames_total")

        servo_render_evaluate_scripts_max_ms = d.pop(
            "servo_render_evaluate_scripts_max_ms"
        )

        servo_render_evaluate_scripts_total_ms = d.pop(
            "servo_render_evaluate_scripts_total_ms"
        )

        servo_render_event_loop_max_ms = d.pop("servo_render_event_loop_max_ms")

        servo_render_event_loop_total_ms = d.pop("servo_render_event_loop_total_ms")

        servo_render_frame_max_ms = d.pop("servo_render_frame_max_ms")

        servo_render_frame_total_ms = d.pop("servo_render_frame_total_ms")

        servo_render_gpu_frames_total = d.pop("servo_render_gpu_frames_total")

        servo_render_paint_max_ms = d.pop("servo_render_paint_max_ms")

        servo_render_paint_total_ms = d.pop("servo_render_paint_total_ms")

        servo_render_queue_wait_max_ms = d.pop("servo_render_queue_wait_max_ms")

        servo_render_queue_wait_total_ms = d.pop("servo_render_queue_wait_total_ms")

        servo_render_readback_max_ms = d.pop("servo_render_readback_max_ms")

        servo_render_readback_total_ms = d.pop("servo_render_readback_total_ms")

        servo_render_requests_total = d.pop("servo_render_requests_total")

        servo_session_create_failures_total = d.pop(
            "servo_session_create_failures_total"
        )

        servo_session_create_wait_max_ms = d.pop("servo_session_create_wait_max_ms")

        servo_session_create_wait_total_ms = d.pop("servo_session_create_wait_total_ms")

        servo_session_creates_total = d.pop("servo_session_creates_total")

        servo_soft_stalls_total = d.pop("servo_soft_stalls_total")

        sparkleflinger_gpu_source_upload_skipped_total = d.pop(
            "sparkleflinger_gpu_source_upload_skipped_total"
        )

        def _parse_servo_gpu_import_fallback_reason(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        servo_gpu_import_fallback_reason = _parse_servo_gpu_import_fallback_reason(
            d.pop("servo_gpu_import_fallback_reason", UNSET)
        )

        effect_health_status = cls(
            errors_total=errors_total,
            fallbacks_applied_total=fallbacks_applied_total,
            producer_cpu_frames_total=producer_cpu_frames_total,
            producer_gpu_frames_total=producer_gpu_frames_total,
            servo_breaker_opens_total=servo_breaker_opens_total,
            servo_detached_destroy_failures_total=servo_detached_destroy_failures_total,
            servo_detached_destroys_total=servo_detached_destroys_total,
            servo_gpu_import_blit_max_ms=servo_gpu_import_blit_max_ms,
            servo_gpu_import_blit_total_ms=servo_gpu_import_blit_total_ms,
            servo_gpu_import_failures_total=servo_gpu_import_failures_total,
            servo_gpu_import_fallbacks_total=servo_gpu_import_fallbacks_total,
            servo_gpu_import_max_ms=servo_gpu_import_max_ms,
            servo_gpu_import_sync_max_ms=servo_gpu_import_sync_max_ms,
            servo_gpu_import_sync_total_ms=servo_gpu_import_sync_total_ms,
            servo_gpu_import_total_ms=servo_gpu_import_total_ms,
            servo_page_load_failures_total=servo_page_load_failures_total,
            servo_page_load_wait_max_ms=servo_page_load_wait_max_ms,
            servo_page_load_wait_total_ms=servo_page_load_wait_total_ms,
            servo_page_loads_total=servo_page_loads_total,
            servo_render_cached_frames_total=servo_render_cached_frames_total,
            servo_render_cpu_frames_total=servo_render_cpu_frames_total,
            servo_render_evaluate_scripts_max_ms=servo_render_evaluate_scripts_max_ms,
            servo_render_evaluate_scripts_total_ms=servo_render_evaluate_scripts_total_ms,
            servo_render_event_loop_max_ms=servo_render_event_loop_max_ms,
            servo_render_event_loop_total_ms=servo_render_event_loop_total_ms,
            servo_render_frame_max_ms=servo_render_frame_max_ms,
            servo_render_frame_total_ms=servo_render_frame_total_ms,
            servo_render_gpu_frames_total=servo_render_gpu_frames_total,
            servo_render_paint_max_ms=servo_render_paint_max_ms,
            servo_render_paint_total_ms=servo_render_paint_total_ms,
            servo_render_queue_wait_max_ms=servo_render_queue_wait_max_ms,
            servo_render_queue_wait_total_ms=servo_render_queue_wait_total_ms,
            servo_render_readback_max_ms=servo_render_readback_max_ms,
            servo_render_readback_total_ms=servo_render_readback_total_ms,
            servo_render_requests_total=servo_render_requests_total,
            servo_session_create_failures_total=servo_session_create_failures_total,
            servo_session_create_wait_max_ms=servo_session_create_wait_max_ms,
            servo_session_create_wait_total_ms=servo_session_create_wait_total_ms,
            servo_session_creates_total=servo_session_creates_total,
            servo_soft_stalls_total=servo_soft_stalls_total,
            sparkleflinger_gpu_source_upload_skipped_total=sparkleflinger_gpu_source_upload_skipped_total,
            servo_gpu_import_fallback_reason=servo_gpu_import_fallback_reason,
        )

        effect_health_status.additional_properties = d
        return effect_health_status

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
