from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.display_class import DisplayClass
from ..models.display_pixel_format import DisplayPixelFormat
from ..models.display_shape import DisplayShape

if TYPE_CHECKING:
    from ..models.display_rect import DisplayRect


T = TypeVar("T", bound="DisplayDescriptor")


@_attrs_define
class DisplayDescriptor:
    """Everything a face needs to know about the surface it renders on.

    Attributes:
        api_version (int): Contract version for the injected JS view; additive-only.
        circular (bool):
        class_ (DisplayClass): Device family the display belongs to, for layout idiom selection.
        height (int):
        pixel_format (DisplayPixelFormat): Pixel format the device transport expects.
        safe_area (DisplayRect): Pixel rectangle within a display surface.
        shape (DisplayShape): Broad shape classification a face adapts its layout to.
        target_fps (int):
        width (int):
    """

    api_version: int
    circular: bool
    class_: DisplayClass
    height: int
    pixel_format: DisplayPixelFormat
    safe_area: DisplayRect
    shape: DisplayShape
    target_fps: int
    width: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        api_version = self.api_version

        circular = self.circular

        class_ = self.class_.value

        height = self.height

        pixel_format = self.pixel_format.value

        safe_area = self.safe_area.to_dict()

        shape = self.shape.value

        target_fps = self.target_fps

        width = self.width

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "api_version": api_version,
                "circular": circular,
                "class": class_,
                "height": height,
                "pixel_format": pixel_format,
                "safe_area": safe_area,
                "shape": shape,
                "target_fps": target_fps,
                "width": width,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.display_rect import DisplayRect

        d = dict(src_dict)
        api_version = d.pop("api_version")

        circular = d.pop("circular")

        class_ = DisplayClass(d.pop("class"))

        height = d.pop("height")

        pixel_format = DisplayPixelFormat(d.pop("pixel_format"))

        safe_area = DisplayRect.from_dict(d.pop("safe_area"))

        shape = DisplayShape(d.pop("shape"))

        target_fps = d.pop("target_fps")

        width = d.pop("width")

        display_descriptor = cls(
            api_version=api_version,
            circular=circular,
            class_=class_,
            height=height,
            pixel_format=pixel_format,
            safe_area=safe_area,
            shape=shape,
            target_fps=target_fps,
            width=width,
        )

        display_descriptor.additional_properties = d
        return display_descriptor

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
