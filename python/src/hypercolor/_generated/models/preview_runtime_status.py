from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.preview_demand_status import PreviewDemandStatus


T = TypeVar("T", bound="PreviewRuntimeStatus")


@_attrs_define
class PreviewRuntimeStatus:
    """
    Attributes:
        canvas_demand (PreviewDemandStatus):
        canvas_frames_published (int):
        canvas_receivers (int):
        latest_canvas_frame_number (int):
        latest_scene_canvas_frame_number (int):
        latest_screen_canvas_frame_number (int):
        latest_zone_preview_frame_number (int):
        scene_canvas_demand (PreviewDemandStatus):
        scene_canvas_frames_published (int):
        scene_canvas_receivers (int):
        screen_canvas_demand (PreviewDemandStatus):
        screen_canvas_frames_published (int):
        screen_canvas_receivers (int):
        zone_preview_demand (PreviewDemandStatus):
        zone_preview_frames_published (int):
        zone_preview_receivers (int):
    """

    canvas_demand: PreviewDemandStatus
    canvas_frames_published: int
    canvas_receivers: int
    latest_canvas_frame_number: int
    latest_scene_canvas_frame_number: int
    latest_screen_canvas_frame_number: int
    latest_zone_preview_frame_number: int
    scene_canvas_demand: PreviewDemandStatus
    scene_canvas_frames_published: int
    scene_canvas_receivers: int
    screen_canvas_demand: PreviewDemandStatus
    screen_canvas_frames_published: int
    screen_canvas_receivers: int
    zone_preview_demand: PreviewDemandStatus
    zone_preview_frames_published: int
    zone_preview_receivers: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        canvas_demand = self.canvas_demand.to_dict()

        canvas_frames_published = self.canvas_frames_published

        canvas_receivers = self.canvas_receivers

        latest_canvas_frame_number = self.latest_canvas_frame_number

        latest_scene_canvas_frame_number = self.latest_scene_canvas_frame_number

        latest_screen_canvas_frame_number = self.latest_screen_canvas_frame_number

        latest_zone_preview_frame_number = self.latest_zone_preview_frame_number

        scene_canvas_demand = self.scene_canvas_demand.to_dict()

        scene_canvas_frames_published = self.scene_canvas_frames_published

        scene_canvas_receivers = self.scene_canvas_receivers

        screen_canvas_demand = self.screen_canvas_demand.to_dict()

        screen_canvas_frames_published = self.screen_canvas_frames_published

        screen_canvas_receivers = self.screen_canvas_receivers

        zone_preview_demand = self.zone_preview_demand.to_dict()

        zone_preview_frames_published = self.zone_preview_frames_published

        zone_preview_receivers = self.zone_preview_receivers

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "canvas_demand": canvas_demand,
                "canvas_frames_published": canvas_frames_published,
                "canvas_receivers": canvas_receivers,
                "latest_canvas_frame_number": latest_canvas_frame_number,
                "latest_scene_canvas_frame_number": latest_scene_canvas_frame_number,
                "latest_screen_canvas_frame_number": latest_screen_canvas_frame_number,
                "latest_zone_preview_frame_number": latest_zone_preview_frame_number,
                "scene_canvas_demand": scene_canvas_demand,
                "scene_canvas_frames_published": scene_canvas_frames_published,
                "scene_canvas_receivers": scene_canvas_receivers,
                "screen_canvas_demand": screen_canvas_demand,
                "screen_canvas_frames_published": screen_canvas_frames_published,
                "screen_canvas_receivers": screen_canvas_receivers,
                "zone_preview_demand": zone_preview_demand,
                "zone_preview_frames_published": zone_preview_frames_published,
                "zone_preview_receivers": zone_preview_receivers,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.preview_demand_status import PreviewDemandStatus

        d = dict(src_dict)
        canvas_demand = PreviewDemandStatus.from_dict(d.pop("canvas_demand"))

        canvas_frames_published = d.pop("canvas_frames_published")

        canvas_receivers = d.pop("canvas_receivers")

        latest_canvas_frame_number = d.pop("latest_canvas_frame_number")

        latest_scene_canvas_frame_number = d.pop("latest_scene_canvas_frame_number")

        latest_screen_canvas_frame_number = d.pop("latest_screen_canvas_frame_number")

        latest_zone_preview_frame_number = d.pop("latest_zone_preview_frame_number")

        scene_canvas_demand = PreviewDemandStatus.from_dict(
            d.pop("scene_canvas_demand")
        )

        scene_canvas_frames_published = d.pop("scene_canvas_frames_published")

        scene_canvas_receivers = d.pop("scene_canvas_receivers")

        screen_canvas_demand = PreviewDemandStatus.from_dict(
            d.pop("screen_canvas_demand")
        )

        screen_canvas_frames_published = d.pop("screen_canvas_frames_published")

        screen_canvas_receivers = d.pop("screen_canvas_receivers")

        zone_preview_demand = PreviewDemandStatus.from_dict(
            d.pop("zone_preview_demand")
        )

        zone_preview_frames_published = d.pop("zone_preview_frames_published")

        zone_preview_receivers = d.pop("zone_preview_receivers")

        preview_runtime_status = cls(
            canvas_demand=canvas_demand,
            canvas_frames_published=canvas_frames_published,
            canvas_receivers=canvas_receivers,
            latest_canvas_frame_number=latest_canvas_frame_number,
            latest_scene_canvas_frame_number=latest_scene_canvas_frame_number,
            latest_screen_canvas_frame_number=latest_screen_canvas_frame_number,
            latest_zone_preview_frame_number=latest_zone_preview_frame_number,
            scene_canvas_demand=scene_canvas_demand,
            scene_canvas_frames_published=scene_canvas_frames_published,
            scene_canvas_receivers=scene_canvas_receivers,
            screen_canvas_demand=screen_canvas_demand,
            screen_canvas_frames_published=screen_canvas_frames_published,
            screen_canvas_receivers=screen_canvas_receivers,
            zone_preview_demand=zone_preview_demand,
            zone_preview_frames_published=zone_preview_frames_published,
            zone_preview_receivers=zone_preview_receivers,
        )

        preview_runtime_status.additional_properties = d
        return preview_runtime_status

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
