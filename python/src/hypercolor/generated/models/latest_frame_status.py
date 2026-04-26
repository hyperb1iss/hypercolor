from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.render_surface_status import RenderSurfaceStatus


T = TypeVar("T", bound="LatestFrameStatus")


@_attrs_define
class LatestFrameStatus:
    """
    Attributes:
        composition_ms (float):
        compositor_backend (str):
        coordination_overhead_ms (float):
        cpu_readback_skipped (bool):
        cpu_sampling_late_readback (bool):
        device_output_ms (float):
        effect_rendering_ms (float):
        event_bus_ms (float):
        frame_age_ms (float):
        frame_token (int):
        full_frame_copy_count (int):
        full_frame_copy_kb (float):
        gpu_sample_cpu_fallback (bool):
        gpu_sample_deferred (bool):
        gpu_sample_queue_saturated (bool):
        gpu_sample_retry_hit (bool):
        gpu_sample_stale (bool):
        gpu_sample_wait_blocked (bool):
        gpu_zone_sampling (bool):
        input_sampling_ms (float):
        jitter_ms (float):
        logical_layer_count (int):
        output_errors (int):
        preview_postprocess_ms (float):
        producer_ms (float):
        producer_preview_compose_ms (float):
        producer_render_ms (float):
        publish_events_ms (float):
        publish_frame_data_ms (float):
        publish_group_canvas_ms (float):
        publish_preview_ms (float):
        render_group_count (int):
        render_surfaces (RenderSurfaceStatus):
        spatial_sampling_ms (float):
        total_ms (float):
        wake_late_ms (float):
    """

    composition_ms: float
    compositor_backend: str
    coordination_overhead_ms: float
    cpu_readback_skipped: bool
    cpu_sampling_late_readback: bool
    device_output_ms: float
    effect_rendering_ms: float
    event_bus_ms: float
    frame_age_ms: float
    frame_token: int
    full_frame_copy_count: int
    full_frame_copy_kb: float
    gpu_sample_cpu_fallback: bool
    gpu_sample_deferred: bool
    gpu_sample_queue_saturated: bool
    gpu_sample_retry_hit: bool
    gpu_sample_stale: bool
    gpu_sample_wait_blocked: bool
    gpu_zone_sampling: bool
    input_sampling_ms: float
    jitter_ms: float
    logical_layer_count: int
    output_errors: int
    preview_postprocess_ms: float
    producer_ms: float
    producer_preview_compose_ms: float
    producer_render_ms: float
    publish_events_ms: float
    publish_frame_data_ms: float
    publish_group_canvas_ms: float
    publish_preview_ms: float
    render_group_count: int
    render_surfaces: RenderSurfaceStatus
    spatial_sampling_ms: float
    total_ms: float
    wake_late_ms: float
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        composition_ms = self.composition_ms

        compositor_backend = self.compositor_backend

        coordination_overhead_ms = self.coordination_overhead_ms

        cpu_readback_skipped = self.cpu_readback_skipped

        cpu_sampling_late_readback = self.cpu_sampling_late_readback

        device_output_ms = self.device_output_ms

        effect_rendering_ms = self.effect_rendering_ms

        event_bus_ms = self.event_bus_ms

        frame_age_ms = self.frame_age_ms

        frame_token = self.frame_token

        full_frame_copy_count = self.full_frame_copy_count

        full_frame_copy_kb = self.full_frame_copy_kb

        gpu_sample_cpu_fallback = self.gpu_sample_cpu_fallback

        gpu_sample_deferred = self.gpu_sample_deferred

        gpu_sample_queue_saturated = self.gpu_sample_queue_saturated

        gpu_sample_retry_hit = self.gpu_sample_retry_hit

        gpu_sample_stale = self.gpu_sample_stale

        gpu_sample_wait_blocked = self.gpu_sample_wait_blocked

        gpu_zone_sampling = self.gpu_zone_sampling

        input_sampling_ms = self.input_sampling_ms

        jitter_ms = self.jitter_ms

        logical_layer_count = self.logical_layer_count

        output_errors = self.output_errors

        preview_postprocess_ms = self.preview_postprocess_ms

        producer_ms = self.producer_ms

        producer_preview_compose_ms = self.producer_preview_compose_ms

        producer_render_ms = self.producer_render_ms

        publish_events_ms = self.publish_events_ms

        publish_frame_data_ms = self.publish_frame_data_ms

        publish_group_canvas_ms = self.publish_group_canvas_ms

        publish_preview_ms = self.publish_preview_ms

        render_group_count = self.render_group_count

        render_surfaces = self.render_surfaces.to_dict()

        spatial_sampling_ms = self.spatial_sampling_ms

        total_ms = self.total_ms

        wake_late_ms = self.wake_late_ms

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "composition_ms": composition_ms,
                "compositor_backend": compositor_backend,
                "coordination_overhead_ms": coordination_overhead_ms,
                "cpu_readback_skipped": cpu_readback_skipped,
                "cpu_sampling_late_readback": cpu_sampling_late_readback,
                "device_output_ms": device_output_ms,
                "effect_rendering_ms": effect_rendering_ms,
                "event_bus_ms": event_bus_ms,
                "frame_age_ms": frame_age_ms,
                "frame_token": frame_token,
                "full_frame_copy_count": full_frame_copy_count,
                "full_frame_copy_kb": full_frame_copy_kb,
                "gpu_sample_cpu_fallback": gpu_sample_cpu_fallback,
                "gpu_sample_deferred": gpu_sample_deferred,
                "gpu_sample_queue_saturated": gpu_sample_queue_saturated,
                "gpu_sample_retry_hit": gpu_sample_retry_hit,
                "gpu_sample_stale": gpu_sample_stale,
                "gpu_sample_wait_blocked": gpu_sample_wait_blocked,
                "gpu_zone_sampling": gpu_zone_sampling,
                "input_sampling_ms": input_sampling_ms,
                "jitter_ms": jitter_ms,
                "logical_layer_count": logical_layer_count,
                "output_errors": output_errors,
                "preview_postprocess_ms": preview_postprocess_ms,
                "producer_ms": producer_ms,
                "producer_preview_compose_ms": producer_preview_compose_ms,
                "producer_render_ms": producer_render_ms,
                "publish_events_ms": publish_events_ms,
                "publish_frame_data_ms": publish_frame_data_ms,
                "publish_group_canvas_ms": publish_group_canvas_ms,
                "publish_preview_ms": publish_preview_ms,
                "render_group_count": render_group_count,
                "render_surfaces": render_surfaces,
                "spatial_sampling_ms": spatial_sampling_ms,
                "total_ms": total_ms,
                "wake_late_ms": wake_late_ms,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.render_surface_status import RenderSurfaceStatus

        d = dict(src_dict)
        composition_ms = d.pop("composition_ms")

        compositor_backend = d.pop("compositor_backend")

        coordination_overhead_ms = d.pop("coordination_overhead_ms")

        cpu_readback_skipped = d.pop("cpu_readback_skipped")

        cpu_sampling_late_readback = d.pop("cpu_sampling_late_readback")

        device_output_ms = d.pop("device_output_ms")

        effect_rendering_ms = d.pop("effect_rendering_ms")

        event_bus_ms = d.pop("event_bus_ms")

        frame_age_ms = d.pop("frame_age_ms")

        frame_token = d.pop("frame_token")

        full_frame_copy_count = d.pop("full_frame_copy_count")

        full_frame_copy_kb = d.pop("full_frame_copy_kb")

        gpu_sample_cpu_fallback = d.pop("gpu_sample_cpu_fallback")

        gpu_sample_deferred = d.pop("gpu_sample_deferred")

        gpu_sample_queue_saturated = d.pop("gpu_sample_queue_saturated")

        gpu_sample_retry_hit = d.pop("gpu_sample_retry_hit")

        gpu_sample_stale = d.pop("gpu_sample_stale")

        gpu_sample_wait_blocked = d.pop("gpu_sample_wait_blocked")

        gpu_zone_sampling = d.pop("gpu_zone_sampling")

        input_sampling_ms = d.pop("input_sampling_ms")

        jitter_ms = d.pop("jitter_ms")

        logical_layer_count = d.pop("logical_layer_count")

        output_errors = d.pop("output_errors")

        preview_postprocess_ms = d.pop("preview_postprocess_ms")

        producer_ms = d.pop("producer_ms")

        producer_preview_compose_ms = d.pop("producer_preview_compose_ms")

        producer_render_ms = d.pop("producer_render_ms")

        publish_events_ms = d.pop("publish_events_ms")

        publish_frame_data_ms = d.pop("publish_frame_data_ms")

        publish_group_canvas_ms = d.pop("publish_group_canvas_ms")

        publish_preview_ms = d.pop("publish_preview_ms")

        render_group_count = d.pop("render_group_count")

        render_surfaces = RenderSurfaceStatus.from_dict(d.pop("render_surfaces"))

        spatial_sampling_ms = d.pop("spatial_sampling_ms")

        total_ms = d.pop("total_ms")

        wake_late_ms = d.pop("wake_late_ms")

        latest_frame_status = cls(
            composition_ms=composition_ms,
            compositor_backend=compositor_backend,
            coordination_overhead_ms=coordination_overhead_ms,
            cpu_readback_skipped=cpu_readback_skipped,
            cpu_sampling_late_readback=cpu_sampling_late_readback,
            device_output_ms=device_output_ms,
            effect_rendering_ms=effect_rendering_ms,
            event_bus_ms=event_bus_ms,
            frame_age_ms=frame_age_ms,
            frame_token=frame_token,
            full_frame_copy_count=full_frame_copy_count,
            full_frame_copy_kb=full_frame_copy_kb,
            gpu_sample_cpu_fallback=gpu_sample_cpu_fallback,
            gpu_sample_deferred=gpu_sample_deferred,
            gpu_sample_queue_saturated=gpu_sample_queue_saturated,
            gpu_sample_retry_hit=gpu_sample_retry_hit,
            gpu_sample_stale=gpu_sample_stale,
            gpu_sample_wait_blocked=gpu_sample_wait_blocked,
            gpu_zone_sampling=gpu_zone_sampling,
            input_sampling_ms=input_sampling_ms,
            jitter_ms=jitter_ms,
            logical_layer_count=logical_layer_count,
            output_errors=output_errors,
            preview_postprocess_ms=preview_postprocess_ms,
            producer_ms=producer_ms,
            producer_preview_compose_ms=producer_preview_compose_ms,
            producer_render_ms=producer_render_ms,
            publish_events_ms=publish_events_ms,
            publish_frame_data_ms=publish_frame_data_ms,
            publish_group_canvas_ms=publish_group_canvas_ms,
            publish_preview_ms=publish_preview_ms,
            render_group_count=render_group_count,
            render_surfaces=render_surfaces,
            spatial_sampling_ms=spatial_sampling_ms,
            total_ms=total_ms,
            wake_late_ms=wake_late_ms,
        )

        latest_frame_status.additional_properties = d
        return latest_frame_status

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
