from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..types import UNSET, Unset

T = TypeVar("T", bound="RenderOutputDemandStatus")


@_attrs_define
class RenderOutputDemandStatus:
    """Output demand: what consumes the render loop's frames.

    The render loop renders only while something consumes its output: a
    connected device, a display, a preview or canvas stream, a capture, or
    an explicit demand hold. With none of those it parks, renders nothing,
    and resumes at its full tier the moment a consumer appears.

        Attributes:
            demand_holds (int): Explicit demand holds, such as display output for previewed
                simulated displays.
            input_event_streams (int): Stream and diagnostic consumers of host input events, which the
                render loop republishes.
            output_devices (int): Connected devices that receive frames. Simulated displays count only
                while previewed, through `demand_holds`.
            parked (bool): Whether the render loop is parked because nothing consumes its frames.
            parks_total (int): How many times the loop has parked since the daemon started.
            stream_receivers (int): Live receivers on the frame, canvas, preview, spectrum, and per-zone
                canvas streams, including in-flight canvas captures.
            idle_reason (None | str | Unset): Why the render loop is producing no frames, when it is parked.
                `no_output_consumers` while parked.
            last_wake_reason (None | str | Unset): What last woke the loop from a park: `output_consumer`,
                `scene_transaction`, `producer_state`, or `render_loop_state`.
            parked_ms (int | None | Unset): How long the loop has been parked, in milliseconds.
    """

    demand_holds: int
    input_event_streams: int
    output_devices: int
    parked: bool
    parks_total: int
    stream_receivers: int
    idle_reason: None | str | Unset = UNSET
    last_wake_reason: None | str | Unset = UNSET
    parked_ms: int | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        demand_holds = self.demand_holds

        input_event_streams = self.input_event_streams

        output_devices = self.output_devices

        parked = self.parked

        parks_total = self.parks_total

        stream_receivers = self.stream_receivers

        idle_reason: None | str | Unset
        if isinstance(self.idle_reason, Unset):
            idle_reason = UNSET
        else:
            idle_reason = self.idle_reason

        last_wake_reason: None | str | Unset
        if isinstance(self.last_wake_reason, Unset):
            last_wake_reason = UNSET
        else:
            last_wake_reason = self.last_wake_reason

        parked_ms: int | None | Unset
        if isinstance(self.parked_ms, Unset):
            parked_ms = UNSET
        else:
            parked_ms = self.parked_ms

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "demand_holds": demand_holds,
                "input_event_streams": input_event_streams,
                "output_devices": output_devices,
                "parked": parked,
                "parks_total": parks_total,
                "stream_receivers": stream_receivers,
            }
        )
        if idle_reason is not UNSET:
            field_dict["idle_reason"] = idle_reason
        if last_wake_reason is not UNSET:
            field_dict["last_wake_reason"] = last_wake_reason
        if parked_ms is not UNSET:
            field_dict["parked_ms"] = parked_ms

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        demand_holds = d.pop("demand_holds")

        input_event_streams = d.pop("input_event_streams")

        output_devices = d.pop("output_devices")

        parked = d.pop("parked")

        parks_total = d.pop("parks_total")

        stream_receivers = d.pop("stream_receivers")

        def _parse_idle_reason(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        idle_reason = _parse_idle_reason(d.pop("idle_reason", UNSET))

        def _parse_last_wake_reason(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        last_wake_reason = _parse_last_wake_reason(d.pop("last_wake_reason", UNSET))

        def _parse_parked_ms(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        parked_ms = _parse_parked_ms(d.pop("parked_ms", UNSET))

        render_output_demand_status = cls(
            demand_holds=demand_holds,
            input_event_streams=input_event_streams,
            output_devices=output_devices,
            parked=parked,
            parks_total=parks_total,
            stream_receivers=stream_receivers,
            idle_reason=idle_reason,
            last_wake_reason=last_wake_reason,
            parked_ms=parked_ms,
        )

        render_output_demand_status.additional_properties = d
        return render_output_demand_status

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
