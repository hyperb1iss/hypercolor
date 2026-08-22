from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

T = TypeVar("T", bound="DiagnoseRenderWindowSnapshot")


@_attrs_define
class DiagnoseRenderWindowSnapshot:
    """
    Attributes:
        frames (int):
        gpu_sample_cpu_fallback (int):
        gpu_sample_deferred (int):
        gpu_sample_queue_saturated (int):
        gpu_sample_retry_hit (int):
        gpu_sample_stale (int):
        gpu_sample_wait_blocked (int):
        output_current_frame (int):
        output_error_frames (int):
        output_published_frame (int):
        output_reused_published_frame (int):
        output_routed_reuse (int):
        publish_avg_ms (float):
        publish_p95_ms (float):
        push_avg_ms (float):
        push_p95_ms (float):
    """

    frames: int
    gpu_sample_cpu_fallback: int
    gpu_sample_deferred: int
    gpu_sample_queue_saturated: int
    gpu_sample_retry_hit: int
    gpu_sample_stale: int
    gpu_sample_wait_blocked: int
    output_current_frame: int
    output_error_frames: int
    output_published_frame: int
    output_reused_published_frame: int
    output_routed_reuse: int
    publish_avg_ms: float
    publish_p95_ms: float
    push_avg_ms: float
    push_p95_ms: float
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        frames = self.frames

        gpu_sample_cpu_fallback = self.gpu_sample_cpu_fallback

        gpu_sample_deferred = self.gpu_sample_deferred

        gpu_sample_queue_saturated = self.gpu_sample_queue_saturated

        gpu_sample_retry_hit = self.gpu_sample_retry_hit

        gpu_sample_stale = self.gpu_sample_stale

        gpu_sample_wait_blocked = self.gpu_sample_wait_blocked

        output_current_frame = self.output_current_frame

        output_error_frames = self.output_error_frames

        output_published_frame = self.output_published_frame

        output_reused_published_frame = self.output_reused_published_frame

        output_routed_reuse = self.output_routed_reuse

        publish_avg_ms = self.publish_avg_ms

        publish_p95_ms = self.publish_p95_ms

        push_avg_ms = self.push_avg_ms

        push_p95_ms = self.push_p95_ms

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "frames": frames,
                "gpu_sample_cpu_fallback": gpu_sample_cpu_fallback,
                "gpu_sample_deferred": gpu_sample_deferred,
                "gpu_sample_queue_saturated": gpu_sample_queue_saturated,
                "gpu_sample_retry_hit": gpu_sample_retry_hit,
                "gpu_sample_stale": gpu_sample_stale,
                "gpu_sample_wait_blocked": gpu_sample_wait_blocked,
                "output_current_frame": output_current_frame,
                "output_error_frames": output_error_frames,
                "output_published_frame": output_published_frame,
                "output_reused_published_frame": output_reused_published_frame,
                "output_routed_reuse": output_routed_reuse,
                "publish_avg_ms": publish_avg_ms,
                "publish_p95_ms": publish_p95_ms,
                "push_avg_ms": push_avg_ms,
                "push_p95_ms": push_p95_ms,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        frames = d.pop("frames")

        gpu_sample_cpu_fallback = d.pop("gpu_sample_cpu_fallback")

        gpu_sample_deferred = d.pop("gpu_sample_deferred")

        gpu_sample_queue_saturated = d.pop("gpu_sample_queue_saturated")

        gpu_sample_retry_hit = d.pop("gpu_sample_retry_hit")

        gpu_sample_stale = d.pop("gpu_sample_stale")

        gpu_sample_wait_blocked = d.pop("gpu_sample_wait_blocked")

        output_current_frame = d.pop("output_current_frame")

        output_error_frames = d.pop("output_error_frames")

        output_published_frame = d.pop("output_published_frame")

        output_reused_published_frame = d.pop("output_reused_published_frame")

        output_routed_reuse = d.pop("output_routed_reuse")

        publish_avg_ms = d.pop("publish_avg_ms")

        publish_p95_ms = d.pop("publish_p95_ms")

        push_avg_ms = d.pop("push_avg_ms")

        push_p95_ms = d.pop("push_p95_ms")

        diagnose_render_window_snapshot = cls(
            frames=frames,
            gpu_sample_cpu_fallback=gpu_sample_cpu_fallback,
            gpu_sample_deferred=gpu_sample_deferred,
            gpu_sample_queue_saturated=gpu_sample_queue_saturated,
            gpu_sample_retry_hit=gpu_sample_retry_hit,
            gpu_sample_stale=gpu_sample_stale,
            gpu_sample_wait_blocked=gpu_sample_wait_blocked,
            output_current_frame=output_current_frame,
            output_error_frames=output_error_frames,
            output_published_frame=output_published_frame,
            output_reused_published_frame=output_reused_published_frame,
            output_routed_reuse=output_routed_reuse,
            publish_avg_ms=publish_avg_ms,
            publish_p95_ms=publish_p95_ms,
            push_avg_ms=push_avg_ms,
            push_p95_ms=push_p95_ms,
        )

        diagnose_render_window_snapshot.additional_properties = d
        return diagnose_render_window_snapshot

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
