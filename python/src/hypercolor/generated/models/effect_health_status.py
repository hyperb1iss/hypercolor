from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

T = TypeVar("T", bound="EffectHealthStatus")


@_attrs_define
class EffectHealthStatus:
    """
    Attributes:
        errors_total (int):
        fallbacks_applied_total (int):
        servo_breaker_opens_total (int):
        servo_detached_destroy_failures_total (int):
        servo_detached_destroys_total (int):
        servo_page_load_failures_total (int):
        servo_page_load_wait_max_ms (float):
        servo_page_load_wait_total_ms (float):
        servo_page_loads_total (int):
        servo_render_queue_wait_max_ms (float):
        servo_render_queue_wait_total_ms (float):
        servo_render_requests_total (int):
        servo_session_create_failures_total (int):
        servo_session_create_wait_max_ms (float):
        servo_session_create_wait_total_ms (float):
        servo_session_creates_total (int):
        servo_soft_stalls_total (int):
    """

    errors_total: int
    fallbacks_applied_total: int
    servo_breaker_opens_total: int
    servo_detached_destroy_failures_total: int
    servo_detached_destroys_total: int
    servo_page_load_failures_total: int
    servo_page_load_wait_max_ms: float
    servo_page_load_wait_total_ms: float
    servo_page_loads_total: int
    servo_render_queue_wait_max_ms: float
    servo_render_queue_wait_total_ms: float
    servo_render_requests_total: int
    servo_session_create_failures_total: int
    servo_session_create_wait_max_ms: float
    servo_session_create_wait_total_ms: float
    servo_session_creates_total: int
    servo_soft_stalls_total: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        errors_total = self.errors_total

        fallbacks_applied_total = self.fallbacks_applied_total

        servo_breaker_opens_total = self.servo_breaker_opens_total

        servo_detached_destroy_failures_total = (
            self.servo_detached_destroy_failures_total
        )

        servo_detached_destroys_total = self.servo_detached_destroys_total

        servo_page_load_failures_total = self.servo_page_load_failures_total

        servo_page_load_wait_max_ms = self.servo_page_load_wait_max_ms

        servo_page_load_wait_total_ms = self.servo_page_load_wait_total_ms

        servo_page_loads_total = self.servo_page_loads_total

        servo_render_queue_wait_max_ms = self.servo_render_queue_wait_max_ms

        servo_render_queue_wait_total_ms = self.servo_render_queue_wait_total_ms

        servo_render_requests_total = self.servo_render_requests_total

        servo_session_create_failures_total = self.servo_session_create_failures_total

        servo_session_create_wait_max_ms = self.servo_session_create_wait_max_ms

        servo_session_create_wait_total_ms = self.servo_session_create_wait_total_ms

        servo_session_creates_total = self.servo_session_creates_total

        servo_soft_stalls_total = self.servo_soft_stalls_total

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "errors_total": errors_total,
                "fallbacks_applied_total": fallbacks_applied_total,
                "servo_breaker_opens_total": servo_breaker_opens_total,
                "servo_detached_destroy_failures_total": servo_detached_destroy_failures_total,
                "servo_detached_destroys_total": servo_detached_destroys_total,
                "servo_page_load_failures_total": servo_page_load_failures_total,
                "servo_page_load_wait_max_ms": servo_page_load_wait_max_ms,
                "servo_page_load_wait_total_ms": servo_page_load_wait_total_ms,
                "servo_page_loads_total": servo_page_loads_total,
                "servo_render_queue_wait_max_ms": servo_render_queue_wait_max_ms,
                "servo_render_queue_wait_total_ms": servo_render_queue_wait_total_ms,
                "servo_render_requests_total": servo_render_requests_total,
                "servo_session_create_failures_total": servo_session_create_failures_total,
                "servo_session_create_wait_max_ms": servo_session_create_wait_max_ms,
                "servo_session_create_wait_total_ms": servo_session_create_wait_total_ms,
                "servo_session_creates_total": servo_session_creates_total,
                "servo_soft_stalls_total": servo_soft_stalls_total,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        errors_total = d.pop("errors_total")

        fallbacks_applied_total = d.pop("fallbacks_applied_total")

        servo_breaker_opens_total = d.pop("servo_breaker_opens_total")

        servo_detached_destroy_failures_total = d.pop(
            "servo_detached_destroy_failures_total"
        )

        servo_detached_destroys_total = d.pop("servo_detached_destroys_total")

        servo_page_load_failures_total = d.pop("servo_page_load_failures_total")

        servo_page_load_wait_max_ms = d.pop("servo_page_load_wait_max_ms")

        servo_page_load_wait_total_ms = d.pop("servo_page_load_wait_total_ms")

        servo_page_loads_total = d.pop("servo_page_loads_total")

        servo_render_queue_wait_max_ms = d.pop("servo_render_queue_wait_max_ms")

        servo_render_queue_wait_total_ms = d.pop("servo_render_queue_wait_total_ms")

        servo_render_requests_total = d.pop("servo_render_requests_total")

        servo_session_create_failures_total = d.pop(
            "servo_session_create_failures_total"
        )

        servo_session_create_wait_max_ms = d.pop("servo_session_create_wait_max_ms")

        servo_session_create_wait_total_ms = d.pop("servo_session_create_wait_total_ms")

        servo_session_creates_total = d.pop("servo_session_creates_total")

        servo_soft_stalls_total = d.pop("servo_soft_stalls_total")

        effect_health_status = cls(
            errors_total=errors_total,
            fallbacks_applied_total=fallbacks_applied_total,
            servo_breaker_opens_total=servo_breaker_opens_total,
            servo_detached_destroy_failures_total=servo_detached_destroy_failures_total,
            servo_detached_destroys_total=servo_detached_destroys_total,
            servo_page_load_failures_total=servo_page_load_failures_total,
            servo_page_load_wait_max_ms=servo_page_load_wait_max_ms,
            servo_page_load_wait_total_ms=servo_page_load_wait_total_ms,
            servo_page_loads_total=servo_page_loads_total,
            servo_render_queue_wait_max_ms=servo_render_queue_wait_max_ms,
            servo_render_queue_wait_total_ms=servo_render_queue_wait_total_ms,
            servo_render_requests_total=servo_render_requests_total,
            servo_session_create_failures_total=servo_session_create_failures_total,
            servo_session_create_wait_max_ms=servo_session_create_wait_max_ms,
            servo_session_create_wait_total_ms=servo_session_create_wait_total_ms,
            servo_session_creates_total=servo_session_creates_total,
            servo_soft_stalls_total=servo_soft_stalls_total,
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
