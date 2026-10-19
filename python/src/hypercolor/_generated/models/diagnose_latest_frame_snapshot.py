from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..types import UNSET, Unset

T = TypeVar("T", bound="DiagnoseLatestFrameSnapshot")


@_attrs_define
class DiagnoseLatestFrameSnapshot:
    """
    Attributes:
        composition_us (int):
        compositor_backend (str):
        cpu_readback_skipped (bool):
        devices_written (int):
        frame_age_ms (float):
        frame_token (int):
        gpu_readback_failed (bool):
        gpu_sample_cpu_fallback (bool):
        gpu_sample_deferred (bool):
        gpu_sample_queue_saturated (bool):
        gpu_sample_retry_hit (bool):
        gpu_sample_stale (bool):
        gpu_sample_wait_blocked (bool):
        gpu_zone_sampling (bool):
        input_us (int):
        output_brightness_bits (int):
        output_brightness_generation (int):
        output_errors (int):
        output_frame_source (str):
        output_reuses_published_frame (bool):
        output_routing_signature (int):
        output_unassigned_behavior_generation (int):
        output_zone_shape_signature (int):
        overhead_us (int):
        producer_us (int):
        publish_us (int):
        push_us (int):
        render_us (int):
        sample_us (int):
        total_leds (int):
        total_us (int):
        cpu_sampled_zone_count (int | Unset): Scene zones whose LEDs sampled a CPU-backed zone frame on the CPU.
            Zero when the frame sampled no per-zone set. Absent from older
            daemons.
        gpu_sampled_zone_count (int | Unset): Scene zones whose LEDs sample their own zone texture on the GPU; the
            `gpu_sample_*` flags say whether this frame's sample landed. Zero
            when the frame sampled no per-zone set (a single-zone scene, or the
            CPU backend). Absent from older daemons.
        gpu_sampling_passes (int | Unset): Texture passes the GPU sampler encoded for the frame's LEDs: one per
            sampled zone texture, plus one when unassigned fallback outputs ride
            along. Zero when the sample cache answered or nothing dispatched.
            Absent from older daemons.
        held_zone_count (int | Unset): Scene zones with no frame to sample, whose LEDs held their published
            colors or went black. Zero when the frame sampled no per-zone set.
            Absent from older daemons.
        zone_composition_skips (int | Unset): Scene zones whose layers matched their held frame, so they skipped
            composing this frame. Absent from older daemons.
    """

    composition_us: int
    compositor_backend: str
    cpu_readback_skipped: bool
    devices_written: int
    frame_age_ms: float
    frame_token: int
    gpu_readback_failed: bool
    gpu_sample_cpu_fallback: bool
    gpu_sample_deferred: bool
    gpu_sample_queue_saturated: bool
    gpu_sample_retry_hit: bool
    gpu_sample_stale: bool
    gpu_sample_wait_blocked: bool
    gpu_zone_sampling: bool
    input_us: int
    output_brightness_bits: int
    output_brightness_generation: int
    output_errors: int
    output_frame_source: str
    output_reuses_published_frame: bool
    output_routing_signature: int
    output_unassigned_behavior_generation: int
    output_zone_shape_signature: int
    overhead_us: int
    producer_us: int
    publish_us: int
    push_us: int
    render_us: int
    sample_us: int
    total_leds: int
    total_us: int
    cpu_sampled_zone_count: int | Unset = UNSET
    gpu_sampled_zone_count: int | Unset = UNSET
    gpu_sampling_passes: int | Unset = UNSET
    held_zone_count: int | Unset = UNSET
    zone_composition_skips: int | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        composition_us = self.composition_us

        compositor_backend = self.compositor_backend

        cpu_readback_skipped = self.cpu_readback_skipped

        devices_written = self.devices_written

        frame_age_ms = self.frame_age_ms

        frame_token = self.frame_token

        gpu_readback_failed = self.gpu_readback_failed

        gpu_sample_cpu_fallback = self.gpu_sample_cpu_fallback

        gpu_sample_deferred = self.gpu_sample_deferred

        gpu_sample_queue_saturated = self.gpu_sample_queue_saturated

        gpu_sample_retry_hit = self.gpu_sample_retry_hit

        gpu_sample_stale = self.gpu_sample_stale

        gpu_sample_wait_blocked = self.gpu_sample_wait_blocked

        gpu_zone_sampling = self.gpu_zone_sampling

        input_us = self.input_us

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

        overhead_us = self.overhead_us

        producer_us = self.producer_us

        publish_us = self.publish_us

        push_us = self.push_us

        render_us = self.render_us

        sample_us = self.sample_us

        total_leds = self.total_leds

        total_us = self.total_us

        cpu_sampled_zone_count = self.cpu_sampled_zone_count

        gpu_sampled_zone_count = self.gpu_sampled_zone_count

        gpu_sampling_passes = self.gpu_sampling_passes

        held_zone_count = self.held_zone_count

        zone_composition_skips = self.zone_composition_skips

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "composition_us": composition_us,
                "compositor_backend": compositor_backend,
                "cpu_readback_skipped": cpu_readback_skipped,
                "devices_written": devices_written,
                "frame_age_ms": frame_age_ms,
                "frame_token": frame_token,
                "gpu_readback_failed": gpu_readback_failed,
                "gpu_sample_cpu_fallback": gpu_sample_cpu_fallback,
                "gpu_sample_deferred": gpu_sample_deferred,
                "gpu_sample_queue_saturated": gpu_sample_queue_saturated,
                "gpu_sample_retry_hit": gpu_sample_retry_hit,
                "gpu_sample_stale": gpu_sample_stale,
                "gpu_sample_wait_blocked": gpu_sample_wait_blocked,
                "gpu_zone_sampling": gpu_zone_sampling,
                "input_us": input_us,
                "output_brightness_bits": output_brightness_bits,
                "output_brightness_generation": output_brightness_generation,
                "output_errors": output_errors,
                "output_frame_source": output_frame_source,
                "output_reuses_published_frame": output_reuses_published_frame,
                "output_routing_signature": output_routing_signature,
                "output_unassigned_behavior_generation": output_unassigned_behavior_generation,
                "output_zone_shape_signature": output_zone_shape_signature,
                "overhead_us": overhead_us,
                "producer_us": producer_us,
                "publish_us": publish_us,
                "push_us": push_us,
                "render_us": render_us,
                "sample_us": sample_us,
                "total_leds": total_leds,
                "total_us": total_us,
            }
        )
        if cpu_sampled_zone_count is not UNSET:
            field_dict["cpu_sampled_zone_count"] = cpu_sampled_zone_count
        if gpu_sampled_zone_count is not UNSET:
            field_dict["gpu_sampled_zone_count"] = gpu_sampled_zone_count
        if gpu_sampling_passes is not UNSET:
            field_dict["gpu_sampling_passes"] = gpu_sampling_passes
        if held_zone_count is not UNSET:
            field_dict["held_zone_count"] = held_zone_count
        if zone_composition_skips is not UNSET:
            field_dict["zone_composition_skips"] = zone_composition_skips

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        composition_us = d.pop("composition_us")

        compositor_backend = d.pop("compositor_backend")

        cpu_readback_skipped = d.pop("cpu_readback_skipped")

        devices_written = d.pop("devices_written")

        frame_age_ms = d.pop("frame_age_ms")

        frame_token = d.pop("frame_token")

        gpu_readback_failed = d.pop("gpu_readback_failed")

        gpu_sample_cpu_fallback = d.pop("gpu_sample_cpu_fallback")

        gpu_sample_deferred = d.pop("gpu_sample_deferred")

        gpu_sample_queue_saturated = d.pop("gpu_sample_queue_saturated")

        gpu_sample_retry_hit = d.pop("gpu_sample_retry_hit")

        gpu_sample_stale = d.pop("gpu_sample_stale")

        gpu_sample_wait_blocked = d.pop("gpu_sample_wait_blocked")

        gpu_zone_sampling = d.pop("gpu_zone_sampling")

        input_us = d.pop("input_us")

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

        overhead_us = d.pop("overhead_us")

        producer_us = d.pop("producer_us")

        publish_us = d.pop("publish_us")

        push_us = d.pop("push_us")

        render_us = d.pop("render_us")

        sample_us = d.pop("sample_us")

        total_leds = d.pop("total_leds")

        total_us = d.pop("total_us")

        cpu_sampled_zone_count = d.pop("cpu_sampled_zone_count", UNSET)

        gpu_sampled_zone_count = d.pop("gpu_sampled_zone_count", UNSET)

        gpu_sampling_passes = d.pop("gpu_sampling_passes", UNSET)

        held_zone_count = d.pop("held_zone_count", UNSET)

        zone_composition_skips = d.pop("zone_composition_skips", UNSET)

        diagnose_latest_frame_snapshot = cls(
            composition_us=composition_us,
            compositor_backend=compositor_backend,
            cpu_readback_skipped=cpu_readback_skipped,
            devices_written=devices_written,
            frame_age_ms=frame_age_ms,
            frame_token=frame_token,
            gpu_readback_failed=gpu_readback_failed,
            gpu_sample_cpu_fallback=gpu_sample_cpu_fallback,
            gpu_sample_deferred=gpu_sample_deferred,
            gpu_sample_queue_saturated=gpu_sample_queue_saturated,
            gpu_sample_retry_hit=gpu_sample_retry_hit,
            gpu_sample_stale=gpu_sample_stale,
            gpu_sample_wait_blocked=gpu_sample_wait_blocked,
            gpu_zone_sampling=gpu_zone_sampling,
            input_us=input_us,
            output_brightness_bits=output_brightness_bits,
            output_brightness_generation=output_brightness_generation,
            output_errors=output_errors,
            output_frame_source=output_frame_source,
            output_reuses_published_frame=output_reuses_published_frame,
            output_routing_signature=output_routing_signature,
            output_unassigned_behavior_generation=output_unassigned_behavior_generation,
            output_zone_shape_signature=output_zone_shape_signature,
            overhead_us=overhead_us,
            producer_us=producer_us,
            publish_us=publish_us,
            push_us=push_us,
            render_us=render_us,
            sample_us=sample_us,
            total_leds=total_leds,
            total_us=total_us,
            cpu_sampled_zone_count=cpu_sampled_zone_count,
            gpu_sampled_zone_count=gpu_sampled_zone_count,
            gpu_sampling_passes=gpu_sampling_passes,
            held_zone_count=held_zone_count,
            zone_composition_skips=zone_composition_skips,
        )

        diagnose_latest_frame_snapshot.additional_properties = d
        return diagnose_latest_frame_snapshot

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
