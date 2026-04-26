from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

T = TypeVar("T", bound="RenderLoopStatus")


@_attrs_define
class RenderLoopStatus:
    """
    Attributes:
        actual_fps (float):
        ceiling_fps (int):
        consecutive_misses (int):
        fps_tier (str):
        state (str):
        target_fps (int):
        total_frames (int):
    """

    actual_fps: float
    ceiling_fps: int
    consecutive_misses: int
    fps_tier: str
    state: str
    target_fps: int
    total_frames: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        actual_fps = self.actual_fps

        ceiling_fps = self.ceiling_fps

        consecutive_misses = self.consecutive_misses

        fps_tier = self.fps_tier

        state = self.state

        target_fps = self.target_fps

        total_frames = self.total_frames

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "actual_fps": actual_fps,
                "ceiling_fps": ceiling_fps,
                "consecutive_misses": consecutive_misses,
                "fps_tier": fps_tier,
                "state": state,
                "target_fps": target_fps,
                "total_frames": total_frames,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        actual_fps = d.pop("actual_fps")

        ceiling_fps = d.pop("ceiling_fps")

        consecutive_misses = d.pop("consecutive_misses")

        fps_tier = d.pop("fps_tier")

        state = d.pop("state")

        target_fps = d.pop("target_fps")

        total_frames = d.pop("total_frames")

        render_loop_status = cls(
            actual_fps=actual_fps,
            ceiling_fps=ceiling_fps,
            consecutive_misses=consecutive_misses,
            fps_tier=fps_tier,
            state=state,
            target_fps=target_fps,
            total_frames=total_frames,
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
