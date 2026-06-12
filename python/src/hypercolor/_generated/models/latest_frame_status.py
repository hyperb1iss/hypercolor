from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

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
        devices_written (int):
        effect_rendering_ms (float):
        event_bus_ms (float):
        frame_age_ms (float):
        frame_token (int):
        full_frame_copy_count (int):
        full_frame_copy_kb (float):
        gpu_readback_failed (bool):
        gpu_sample_cpu_fallback (bool):
        gpu_sample_deferred (bool):
        gpu_sample_queue_saturated (bool):
        gpu_sample_retry_hit (bool):
        gpu_sample_stale (bool):
        gpu_sample_wait_blocked (bool):
        gpu_zone_sampling (bool):
        input_sampling_ms (float):
        jitter_ms (float):
        led_sampling_readback (bool):
        logical_layer_count (int):
        output_brightness_bits (int):
        output_brightness_generation (int):
        output_errors (int):
        output_frame_source (str):
        output_reuses_published_frame (bool):
        output_routing_signature (int):
        output_unassigned_behavior_generation (int):
        output_zone_shape_signature (int):
        preview_postprocess_ms (float):
        preview_surface (bool):
        producer_full_frame_copy_count (int):
        producer_full_frame_copy_kb (float):
        producer_ms (float):
        producer_preview_compose_ms (float):
        producer_render_ms (float):
        publication_full_frame_copy_count (int):
        publication_full_frame_copy_kb (float):
        publish_events_ms (float):
        publish_frame_data_ms (float):
        publish_group_canvas_ms (float):
        publish_preview_ms (float):
        render_group_count (int):
        render_surfaces (RenderSurfaceStatus):
        scene_canvas_forced_surface (bool):
        spatial_sampling_ms (float):
        total_leds (int):
        total_ms (float):
        wake_late_ms (float):
        producer_full_frame_copy_reason (None | str | Unset):
        publication_full_frame_copy_reason (None | str | Unset):
    """

    composition_ms: float
    compositor_backend: str
    coordination_overhead_ms: float
    cpu_readback_skipped: bool
    cpu_sampling_late_readback: bool
    device_output_ms: float
    devices_written: int
    effect_rendering_ms: float
    event_bus_ms: float
    frame_age_ms: float
    frame_token: int
    full_frame_copy_count: int
    full_frame_copy_kb: float
    gpu_readback_failed: bool
    gpu_sample_cpu_fallback: bool
    gpu_sample_deferred: bool
    gpu_sample_queue_saturated: bool
    gpu_sample_retry_hit: bool
    gpu_sample_stale: bool
    gpu_sample_wait_blocked: bool
    gpu_zone_sampling: bool
    input_sampling_ms: float
    jitter_ms: float
    led_sampling_readback: bool
    logical_layer_count: int
    output_brightness_bits: int
    output_brightness_generation: int
    output_errors: int
    output_frame_source: str
    output_reuses_published_frame: bool
    output_routing_signature: int
    output_unassigned_behavior_generation: int
    output_zone_shape_signature: int
    preview_postprocess_ms: float
    preview_surface: bool
    producer_full_frame_copy_count: int
    producer_full_frame_copy_kb: float
    producer_ms: float
    producer_preview_compose_ms: float
    producer_render_ms: float
    publication_full_frame_copy_count: int
    publication_full_frame_copy_kb: float
    publish_events_ms: float
    publish_frame_data_ms: float
    publish_group_canvas_ms: float
    publish_preview_ms: float
    render_group_count: int
    render_surfaces: RenderSurfaceStatus
    scene_canvas_forced_surface: bool
    spatial_sampling_ms: float
    total_leds: int
    total_ms: float
    wake_late_ms: float
    producer_full_frame_copy_reason: None | str | Unset = UNSET
    publication_full_frame_copy_reason: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        composition_ms = self.composition_ms

        compositor_backend = self.compositor_backend

        coordination_overhead_ms = self.coordination_overhead_ms

        cpu_readback_skipped = self.cpu_readback_skipped

        cpu_sampling_late_readback = self.cpu_sampling_late_readback

        device_output_ms = self.device_output_ms

        devices_written = self.devices_written

        effect_rendering_ms = self.effect_rendering_ms

        event_bus_ms = self.event_bus_ms

        frame_age_ms = self.frame_age_ms

        frame_token = self.frame_token

        full_frame_copy_count = self.full_frame_copy_count

        full_frame_copy_kb = self.full_frame_copy_kb

        gpu_readback_failed = self.gpu_readback_failed

        gpu_sample_cpu_fallback = self.gpu_sample_cpu_fallback

        gpu_sample_deferred = self.gpu_sample_deferred

        gpu_sample_queue_saturated = self.gpu_sample_queue_saturated

        gpu_sample_retry_hit = self.gpu_sample_retry_hit

        gpu_sample_stale = self.gpu_sample_stale

        gpu_sample_wait_blocked = self.gpu_sample_wait_blocked

        gpu_zone_sampling = self.gpu_zone_sampling

        input_sampling_ms = self.input_sampling_ms

        jitter_ms = self.jitter_ms

        led_sampling_readback = self.led_sampling_readback

        logical_layer_count = self.logical_layer_count

        output_brightness_bits = self.output_brightness_bits

        output_brightness_generation = self.output_brightness_generation

        output_errors = self.output_errors

        output_frame_source = self.output_frame_source

        output_reuses_published_frame = self.output_reuses_published_frame

        output_routing_signature = self.output_routing_signature

        output_unassigned_behavior_generation = (
            self.output_unassigned_behavior_generation
        )

        output_zone_shape_signature = self.output_zone_shape_signature

        preview_postprocess_ms = self.preview_postprocess_ms

        preview_surface = self.preview_surface

        producer_full_frame_copy_count = self.producer_full_frame_copy_count

        producer_full_frame_copy_kb = self.producer_full_frame_copy_kb

        producer_ms = self.producer_ms

        producer_preview_compose_ms = self.producer_preview_compose_ms

        producer_render_ms = self.producer_render_ms

        publication_full_frame_copy_count = self.publication_full_frame_copy_count

        publication_full_frame_copy_kb = self.publication_full_frame_copy_kb

        publish_events_ms = self.publish_events_ms

        publish_frame_data_ms = self.publish_frame_data_ms

        publish_group_canvas_ms = self.publish_group_canvas_ms

        publish_preview_ms = self.publish_preview_ms

        render_group_count = self.render_group_count

        render_surfaces = self.render_surfaces.to_dict()

        scene_canvas_forced_surface = self.scene_canvas_forced_surface

        spatial_sampling_ms = self.spatial_sampling_ms

        total_leds = self.total_leds

        total_ms = self.total_ms

        wake_late_ms = self.wake_late_ms

        producer_full_frame_copy_reason: None | str | Unset
        if isinstance(self.producer_full_frame_copy_reason, Unset):
            producer_full_frame_copy_reason = UNSET
        else:
            producer_full_frame_copy_reason = self.producer_full_frame_copy_reason

        publication_full_frame_copy_reason: None | str | Unset
        if isinstance(self.publication_full_frame_copy_reason, Unset):
            publication_full_frame_copy_reason = UNSET
        else:
            publication_full_frame_copy_reason = self.publication_full_frame_copy_reason

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
                "devices_written": devices_written,
                "effect_rendering_ms": effect_rendering_ms,
                "event_bus_ms": event_bus_ms,
                "frame_age_ms": frame_age_ms,
                "frame_token": frame_token,
                "full_frame_copy_count": full_frame_copy_count,
                "full_frame_copy_kb": full_frame_copy_kb,
                "gpu_readback_failed": gpu_readback_failed,
                "gpu_sample_cpu_fallback": gpu_sample_cpu_fallback,
                "gpu_sample_deferred": gpu_sample_deferred,
                "gpu_sample_queue_saturated": gpu_sample_queue_saturated,
                "gpu_sample_retry_hit": gpu_sample_retry_hit,
                "gpu_sample_stale": gpu_sample_stale,
                "gpu_sample_wait_blocked": gpu_sample_wait_blocked,
                "gpu_zone_sampling": gpu_zone_sampling,
                "input_sampling_ms": input_sampling_ms,
                "jitter_ms": jitter_ms,
                "led_sampling_readback": led_sampling_readback,
                "logical_layer_count": logical_layer_count,
                "output_brightness_bits": output_brightness_bits,
                "output_brightness_generation": output_brightness_generation,
                "output_errors": output_errors,
                "output_frame_source": output_frame_source,
                "output_reuses_published_frame": output_reuses_published_frame,
                "output_routing_signature": output_routing_signature,
                "output_unassigned_behavior_generation": output_unassigned_behavior_generation,
                "output_zone_shape_signature": output_zone_shape_signature,
                "preview_postprocess_ms": preview_postprocess_ms,
                "preview_surface": preview_surface,
                "producer_full_frame_copy_count": producer_full_frame_copy_count,
                "producer_full_frame_copy_kb": producer_full_frame_copy_kb,
                "producer_ms": producer_ms,
                "producer_preview_compose_ms": producer_preview_compose_ms,
                "producer_render_ms": producer_render_ms,
                "publication_full_frame_copy_count": publication_full_frame_copy_count,
                "publication_full_frame_copy_kb": publication_full_frame_copy_kb,
                "publish_events_ms": publish_events_ms,
                "publish_frame_data_ms": publish_frame_data_ms,
                "publish_group_canvas_ms": publish_group_canvas_ms,
                "publish_preview_ms": publish_preview_ms,
                "render_group_count": render_group_count,
                "render_surfaces": render_surfaces,
                "scene_canvas_forced_surface": scene_canvas_forced_surface,
                "spatial_sampling_ms": spatial_sampling_ms,
                "total_leds": total_leds,
                "total_ms": total_ms,
                "wake_late_ms": wake_late_ms,
            }
        )
        if producer_full_frame_copy_reason is not UNSET:
            field_dict["producer_full_frame_copy_reason"] = (
                producer_full_frame_copy_reason
            )
        if publication_full_frame_copy_reason is not UNSET:
            field_dict["publication_full_frame_copy_reason"] = (
                publication_full_frame_copy_reason
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

        devices_written = d.pop("devices_written")

        effect_rendering_ms = d.pop("effect_rendering_ms")

        event_bus_ms = d.pop("event_bus_ms")

        frame_age_ms = d.pop("frame_age_ms")

        frame_token = d.pop("frame_token")

        full_frame_copy_count = d.pop("full_frame_copy_count")

        full_frame_copy_kb = d.pop("full_frame_copy_kb")

        gpu_readback_failed = d.pop("gpu_readback_failed")

        gpu_sample_cpu_fallback = d.pop("gpu_sample_cpu_fallback")

        gpu_sample_deferred = d.pop("gpu_sample_deferred")

        gpu_sample_queue_saturated = d.pop("gpu_sample_queue_saturated")

        gpu_sample_retry_hit = d.pop("gpu_sample_retry_hit")

        gpu_sample_stale = d.pop("gpu_sample_stale")

        gpu_sample_wait_blocked = d.pop("gpu_sample_wait_blocked")

        gpu_zone_sampling = d.pop("gpu_zone_sampling")

        input_sampling_ms = d.pop("input_sampling_ms")

        jitter_ms = d.pop("jitter_ms")

        led_sampling_readback = d.pop("led_sampling_readback")

        logical_layer_count = d.pop("logical_layer_count")

        output_brightness_bits = d.pop("output_brightness_bits")

        output_brightness_generation = d.pop("output_brightness_generation")

        output_errors = d.pop("output_errors")

        output_frame_source = d.pop("output_frame_source")

        output_reuses_published_frame = d.pop("output_reuses_published_frame")

        output_routing_signature = d.pop("output_routing_signature")

        output_unassigned_behavior_generation = d.pop(
            "output_unassigned_behavior_generation"
        )

        output_zone_shape_signature = d.pop("output_zone_shape_signature")

        preview_postprocess_ms = d.pop("preview_postprocess_ms")

        preview_surface = d.pop("preview_surface")

        producer_full_frame_copy_count = d.pop("producer_full_frame_copy_count")

        producer_full_frame_copy_kb = d.pop("producer_full_frame_copy_kb")

        producer_ms = d.pop("producer_ms")

        producer_preview_compose_ms = d.pop("producer_preview_compose_ms")

        producer_render_ms = d.pop("producer_render_ms")

        publication_full_frame_copy_count = d.pop("publication_full_frame_copy_count")

        publication_full_frame_copy_kb = d.pop("publication_full_frame_copy_kb")

        publish_events_ms = d.pop("publish_events_ms")

        publish_frame_data_ms = d.pop("publish_frame_data_ms")

        publish_group_canvas_ms = d.pop("publish_group_canvas_ms")

        publish_preview_ms = d.pop("publish_preview_ms")

        render_group_count = d.pop("render_group_count")

        render_surfaces = RenderSurfaceStatus.from_dict(d.pop("render_surfaces"))

        scene_canvas_forced_surface = d.pop("scene_canvas_forced_surface")

        spatial_sampling_ms = d.pop("spatial_sampling_ms")

        total_leds = d.pop("total_leds")

        total_ms = d.pop("total_ms")

        wake_late_ms = d.pop("wake_late_ms")

        def _parse_producer_full_frame_copy_reason(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        producer_full_frame_copy_reason = _parse_producer_full_frame_copy_reason(
            d.pop("producer_full_frame_copy_reason", UNSET)
        )

        def _parse_publication_full_frame_copy_reason(
            data: object,
        ) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        publication_full_frame_copy_reason = _parse_publication_full_frame_copy_reason(
            d.pop("publication_full_frame_copy_reason", UNSET)
        )

        latest_frame_status = cls(
            composition_ms=composition_ms,
            compositor_backend=compositor_backend,
            coordination_overhead_ms=coordination_overhead_ms,
            cpu_readback_skipped=cpu_readback_skipped,
            cpu_sampling_late_readback=cpu_sampling_late_readback,
            device_output_ms=device_output_ms,
            devices_written=devices_written,
            effect_rendering_ms=effect_rendering_ms,
            event_bus_ms=event_bus_ms,
            frame_age_ms=frame_age_ms,
            frame_token=frame_token,
            full_frame_copy_count=full_frame_copy_count,
            full_frame_copy_kb=full_frame_copy_kb,
            gpu_readback_failed=gpu_readback_failed,
            gpu_sample_cpu_fallback=gpu_sample_cpu_fallback,
            gpu_sample_deferred=gpu_sample_deferred,
            gpu_sample_queue_saturated=gpu_sample_queue_saturated,
            gpu_sample_retry_hit=gpu_sample_retry_hit,
            gpu_sample_stale=gpu_sample_stale,
            gpu_sample_wait_blocked=gpu_sample_wait_blocked,
            gpu_zone_sampling=gpu_zone_sampling,
            input_sampling_ms=input_sampling_ms,
            jitter_ms=jitter_ms,
            led_sampling_readback=led_sampling_readback,
            logical_layer_count=logical_layer_count,
            output_brightness_bits=output_brightness_bits,
            output_brightness_generation=output_brightness_generation,
            output_errors=output_errors,
            output_frame_source=output_frame_source,
            output_reuses_published_frame=output_reuses_published_frame,
            output_routing_signature=output_routing_signature,
            output_unassigned_behavior_generation=output_unassigned_behavior_generation,
            output_zone_shape_signature=output_zone_shape_signature,
            preview_postprocess_ms=preview_postprocess_ms,
            preview_surface=preview_surface,
            producer_full_frame_copy_count=producer_full_frame_copy_count,
            producer_full_frame_copy_kb=producer_full_frame_copy_kb,
            producer_ms=producer_ms,
            producer_preview_compose_ms=producer_preview_compose_ms,
            producer_render_ms=producer_render_ms,
            publication_full_frame_copy_count=publication_full_frame_copy_count,
            publication_full_frame_copy_kb=publication_full_frame_copy_kb,
            publish_events_ms=publish_events_ms,
            publish_frame_data_ms=publish_frame_data_ms,
            publish_group_canvas_ms=publish_group_canvas_ms,
            publish_preview_ms=publish_preview_ms,
            render_group_count=render_group_count,
            render_surfaces=render_surfaces,
            scene_canvas_forced_surface=scene_canvas_forced_surface,
            spatial_sampling_ms=spatial_sampling_ms,
            total_leds=total_leds,
            total_ms=total_ms,
            wake_late_ms=wake_late_ms,
            producer_full_frame_copy_reason=producer_full_frame_copy_reason,
            publication_full_frame_copy_reason=publication_full_frame_copy_reason,
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
