from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

T = TypeVar("T", bound="PreviewDemandStatus")


@_attrs_define
class PreviewDemandStatus:
    """
    Attributes:
        any_full_resolution (bool):
        any_jpeg (bool):
        any_rgb (bool):
        any_rgba (bool):
        max_fps (int):
        max_height (int):
        max_width (int):
        subscribers (int):
    """

    any_full_resolution: bool
    any_jpeg: bool
    any_rgb: bool
    any_rgba: bool
    max_fps: int
    max_height: int
    max_width: int
    subscribers: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        any_full_resolution = self.any_full_resolution

        any_jpeg = self.any_jpeg

        any_rgb = self.any_rgb

        any_rgba = self.any_rgba

        max_fps = self.max_fps

        max_height = self.max_height

        max_width = self.max_width

        subscribers = self.subscribers

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "any_full_resolution": any_full_resolution,
                "any_jpeg": any_jpeg,
                "any_rgb": any_rgb,
                "any_rgba": any_rgba,
                "max_fps": max_fps,
                "max_height": max_height,
                "max_width": max_width,
                "subscribers": subscribers,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        any_full_resolution = d.pop("any_full_resolution")

        any_jpeg = d.pop("any_jpeg")

        any_rgb = d.pop("any_rgb")

        any_rgba = d.pop("any_rgba")

        max_fps = d.pop("max_fps")

        max_height = d.pop("max_height")

        max_width = d.pop("max_width")

        subscribers = d.pop("subscribers")

        preview_demand_status = cls(
            any_full_resolution=any_full_resolution,
            any_jpeg=any_jpeg,
            any_rgb=any_rgb,
            any_rgba=any_rgba,
            max_fps=max_fps,
            max_height=max_height,
            max_width=max_width,
            subscribers=subscribers,
        )

        preview_demand_status.additional_properties = d
        return preview_demand_status

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
