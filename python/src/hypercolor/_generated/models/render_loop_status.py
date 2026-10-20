from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.render_output_demand_status import RenderOutputDemandStatus


T = TypeVar("T", bound="RenderLoopStatus")


@_attrs_define
class RenderLoopStatus:
    """
    Attributes:
        actual_fps (float):
        capacity_fps (float):
        ceiling_fps (int):
        consecutive_misses (int):
        delivered_fps (float):
        fps_tier (str):
        state (str):
        target_fps (int):
        total_frames (int):
        output_demand (RenderOutputDemandStatus | Unset): Output demand: what consumes the render loop's frames.

            The render loop renders only while something consumes its output: a
            connected device, a display, a preview or canvas stream, a capture, or
            an explicit demand hold. With none of those it parks, renders nothing,
            and resumes at its full tier the moment a consumer appears.
    """

    actual_fps: float
    capacity_fps: float
    ceiling_fps: int
    consecutive_misses: int
    delivered_fps: float
    fps_tier: str
    state: str
    target_fps: int
    total_frames: int
    output_demand: RenderOutputDemandStatus | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        actual_fps = self.actual_fps

        capacity_fps = self.capacity_fps

        ceiling_fps = self.ceiling_fps

        consecutive_misses = self.consecutive_misses

        delivered_fps = self.delivered_fps

        fps_tier = self.fps_tier

        state = self.state

        target_fps = self.target_fps

        total_frames = self.total_frames

        output_demand: dict[str, Any] | Unset = UNSET
        if not isinstance(self.output_demand, Unset):
            output_demand = self.output_demand.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "actual_fps": actual_fps,
                "capacity_fps": capacity_fps,
                "ceiling_fps": ceiling_fps,
                "consecutive_misses": consecutive_misses,
                "delivered_fps": delivered_fps,
                "fps_tier": fps_tier,
                "state": state,
                "target_fps": target_fps,
                "total_frames": total_frames,
            }
        )
        if output_demand is not UNSET:
            field_dict["output_demand"] = output_demand

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.render_output_demand_status import (
            RenderOutputDemandStatus,
        )

        d = dict(src_dict)
        actual_fps = d.pop("actual_fps")

        capacity_fps = d.pop("capacity_fps")

        ceiling_fps = d.pop("ceiling_fps")

        consecutive_misses = d.pop("consecutive_misses")

        delivered_fps = d.pop("delivered_fps")

        fps_tier = d.pop("fps_tier")

        state = d.pop("state")

        target_fps = d.pop("target_fps")

        total_frames = d.pop("total_frames")

        _output_demand = d.pop("output_demand", UNSET)
        output_demand: RenderOutputDemandStatus | Unset
        if isinstance(_output_demand, Unset):
            output_demand = UNSET
        else:
            output_demand = RenderOutputDemandStatus.from_dict(_output_demand)

        render_loop_status = cls(
            actual_fps=actual_fps,
            capacity_fps=capacity_fps,
            ceiling_fps=ceiling_fps,
            consecutive_misses=consecutive_misses,
            delivered_fps=delivered_fps,
            fps_tier=fps_tier,
            state=state,
            target_fps=target_fps,
            total_frames=total_frames,
            output_demand=output_demand,
        )

        render_loop_status.additional_properties = d
        return render_loop_status

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
