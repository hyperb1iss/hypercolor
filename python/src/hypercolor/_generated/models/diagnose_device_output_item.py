from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="DiagnoseDeviceOutputItem")


@_attrs_define
class DiagnoseDeviceOutputItem:
    """
    Attributes:
        accepted (int):
        accepted_fps (float):
        avg_latency_ms (int):
        avg_queue_wait_ms (int):
        avg_transport_latency_ms (int):
        avg_write_ms (int):
        backend_id (str):
        coalesced (int):
        coalesced_backend_overrun (int):
        coalesced_target_cadence (int):
        completed_payload_bytes (int):
        delivered_fps (float):
        display_transport_completed (int):
        display_transport_failed (int):
        display_transport_started (int):
        errors_total (int):
        fps_queued (float):
        fps_sent (float):
        fps_target (int):
        frames_dropped (int):
        frames_received (int):
        frames_sent (int):
        id (str):
        last_sequence (int):
        last_transport_completed_sequence (int):
        last_transport_failed_sequence (int):
        last_transport_started_sequence (int):
        mapped_layout_ids (list[str]):
        queue_generation (int):
        transport_completed (int):
        transport_failed (int):
        transport_started (int):
        uses_frame_sink (bool):
        worker_finished (bool):
        display_queue_generation (int | None | Unset):
        last_error (None | str | Unset):
        last_sent_ago_ms (int | None | Unset):
    """

    accepted: int
    accepted_fps: float
    avg_latency_ms: int
    avg_queue_wait_ms: int
    avg_transport_latency_ms: int
    avg_write_ms: int
    backend_id: str
    coalesced: int
    coalesced_backend_overrun: int
    coalesced_target_cadence: int
    completed_payload_bytes: int
    delivered_fps: float
    display_transport_completed: int
    display_transport_failed: int
    display_transport_started: int
    errors_total: int
    fps_queued: float
    fps_sent: float
    fps_target: int
    frames_dropped: int
    frames_received: int
    frames_sent: int
    id: str
    last_sequence: int
    last_transport_completed_sequence: int
    last_transport_failed_sequence: int
    last_transport_started_sequence: int
    mapped_layout_ids: list[str]
    queue_generation: int
    transport_completed: int
    transport_failed: int
    transport_started: int
    uses_frame_sink: bool
    worker_finished: bool
    display_queue_generation: int | None | Unset = UNSET
    last_error: None | str | Unset = UNSET
    last_sent_ago_ms: int | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        accepted = self.accepted

        accepted_fps = self.accepted_fps

        avg_latency_ms = self.avg_latency_ms

        avg_queue_wait_ms = self.avg_queue_wait_ms

        avg_transport_latency_ms = self.avg_transport_latency_ms

        avg_write_ms = self.avg_write_ms

        backend_id = self.backend_id

        coalesced = self.coalesced

        coalesced_backend_overrun = self.coalesced_backend_overrun

        coalesced_target_cadence = self.coalesced_target_cadence

        completed_payload_bytes = self.completed_payload_bytes

        delivered_fps = self.delivered_fps

        display_transport_completed = self.display_transport_completed

        display_transport_failed = self.display_transport_failed

        display_transport_started = self.display_transport_started

        errors_total = self.errors_total

        fps_queued = self.fps_queued

        fps_sent = self.fps_sent

        fps_target = self.fps_target

        frames_dropped = self.frames_dropped

        frames_received = self.frames_received

        frames_sent = self.frames_sent

        id = self.id

        last_sequence = self.last_sequence

        last_transport_completed_sequence = self.last_transport_completed_sequence

        last_transport_failed_sequence = self.last_transport_failed_sequence

        last_transport_started_sequence = self.last_transport_started_sequence

        mapped_layout_ids = self.mapped_layout_ids

        queue_generation = self.queue_generation

        transport_completed = self.transport_completed

        transport_failed = self.transport_failed

        transport_started = self.transport_started

        uses_frame_sink = self.uses_frame_sink

        worker_finished = self.worker_finished

        display_queue_generation: int | None | Unset
        if isinstance(self.display_queue_generation, Unset):
            display_queue_generation = UNSET
        else:
            display_queue_generation = self.display_queue_generation

        last_error: None | str | Unset
        if isinstance(self.last_error, Unset):
            last_error = UNSET
        else:
            last_error = self.last_error

        last_sent_ago_ms: int | None | Unset
        if isinstance(self.last_sent_ago_ms, Unset):
            last_sent_ago_ms = UNSET
        else:
            last_sent_ago_ms = self.last_sent_ago_ms

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "accepted": accepted,
                "accepted_fps": accepted_fps,
                "avg_latency_ms": avg_latency_ms,
                "avg_queue_wait_ms": avg_queue_wait_ms,
                "avg_transport_latency_ms": avg_transport_latency_ms,
                "avg_write_ms": avg_write_ms,
                "backend_id": backend_id,
                "coalesced": coalesced,
                "coalesced_backend_overrun": coalesced_backend_overrun,
                "coalesced_target_cadence": coalesced_target_cadence,
                "completed_payload_bytes": completed_payload_bytes,
                "delivered_fps": delivered_fps,
                "display_transport_completed": display_transport_completed,
                "display_transport_failed": display_transport_failed,
                "display_transport_started": display_transport_started,
                "errors_total": errors_total,
                "fps_queued": fps_queued,
                "fps_sent": fps_sent,
                "fps_target": fps_target,
                "frames_dropped": frames_dropped,
                "frames_received": frames_received,
                "frames_sent": frames_sent,
                "id": id,
                "last_sequence": last_sequence,
                "last_transport_completed_sequence": last_transport_completed_sequence,
                "last_transport_failed_sequence": last_transport_failed_sequence,
                "last_transport_started_sequence": last_transport_started_sequence,
                "mapped_layout_ids": mapped_layout_ids,
                "queue_generation": queue_generation,
                "transport_completed": transport_completed,
                "transport_failed": transport_failed,
                "transport_started": transport_started,
                "uses_frame_sink": uses_frame_sink,
                "worker_finished": worker_finished,
            }
        )
        if display_queue_generation is not UNSET:
            field_dict["display_queue_generation"] = display_queue_generation
        if last_error is not UNSET:
            field_dict["last_error"] = last_error
        if last_sent_ago_ms is not UNSET:
            field_dict["last_sent_ago_ms"] = last_sent_ago_ms

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        accepted = d.pop("accepted")

        accepted_fps = d.pop("accepted_fps")

        avg_latency_ms = d.pop("avg_latency_ms")

        avg_queue_wait_ms = d.pop("avg_queue_wait_ms")

        avg_transport_latency_ms = d.pop("avg_transport_latency_ms")

        avg_write_ms = d.pop("avg_write_ms")

        backend_id = d.pop("backend_id")

        coalesced = d.pop("coalesced")

        coalesced_backend_overrun = d.pop("coalesced_backend_overrun")

        coalesced_target_cadence = d.pop("coalesced_target_cadence")

        completed_payload_bytes = d.pop("completed_payload_bytes")

        delivered_fps = d.pop("delivered_fps")

        display_transport_completed = d.pop("display_transport_completed")

        display_transport_failed = d.pop("display_transport_failed")

        display_transport_started = d.pop("display_transport_started")

        errors_total = d.pop("errors_total")

        fps_queued = d.pop("fps_queued")

        fps_sent = d.pop("fps_sent")

        fps_target = d.pop("fps_target")

        frames_dropped = d.pop("frames_dropped")

        frames_received = d.pop("frames_received")

        frames_sent = d.pop("frames_sent")

        id = d.pop("id")

        last_sequence = d.pop("last_sequence")

        last_transport_completed_sequence = d.pop("last_transport_completed_sequence")

        last_transport_failed_sequence = d.pop("last_transport_failed_sequence")

        last_transport_started_sequence = d.pop("last_transport_started_sequence")

        mapped_layout_ids = cast(list[str], d.pop("mapped_layout_ids"))

        queue_generation = d.pop("queue_generation")

        transport_completed = d.pop("transport_completed")

        transport_failed = d.pop("transport_failed")

        transport_started = d.pop("transport_started")

        uses_frame_sink = d.pop("uses_frame_sink")

        worker_finished = d.pop("worker_finished")

        def _parse_display_queue_generation(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        display_queue_generation = _parse_display_queue_generation(
            d.pop("display_queue_generation", UNSET)
        )

        def _parse_last_error(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        last_error = _parse_last_error(d.pop("last_error", UNSET))

        def _parse_last_sent_ago_ms(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        last_sent_ago_ms = _parse_last_sent_ago_ms(d.pop("last_sent_ago_ms", UNSET))

        diagnose_device_output_item = cls(
            accepted=accepted,
            accepted_fps=accepted_fps,
            avg_latency_ms=avg_latency_ms,
            avg_queue_wait_ms=avg_queue_wait_ms,
            avg_transport_latency_ms=avg_transport_latency_ms,
            avg_write_ms=avg_write_ms,
            backend_id=backend_id,
            coalesced=coalesced,
            coalesced_backend_overrun=coalesced_backend_overrun,
            coalesced_target_cadence=coalesced_target_cadence,
            completed_payload_bytes=completed_payload_bytes,
            delivered_fps=delivered_fps,
            display_transport_completed=display_transport_completed,
            display_transport_failed=display_transport_failed,
            display_transport_started=display_transport_started,
            errors_total=errors_total,
            fps_queued=fps_queued,
            fps_sent=fps_sent,
            fps_target=fps_target,
            frames_dropped=frames_dropped,
            frames_received=frames_received,
            frames_sent=frames_sent,
            id=id,
            last_sequence=last_sequence,
            last_transport_completed_sequence=last_transport_completed_sequence,
            last_transport_failed_sequence=last_transport_failed_sequence,
            last_transport_started_sequence=last_transport_started_sequence,
            mapped_layout_ids=mapped_layout_ids,
            queue_generation=queue_generation,
            transport_completed=transport_completed,
            transport_failed=transport_failed,
            transport_started=transport_started,
            uses_frame_sink=uses_frame_sink,
            worker_finished=worker_finished,
            display_queue_generation=display_queue_generation,
            last_error=last_error,
            last_sent_ago_ms=last_sent_ago_ms,
        )

        diagnose_device_output_item.additional_properties = d
        return diagnose_device_output_item

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
