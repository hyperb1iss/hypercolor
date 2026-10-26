from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

T = TypeVar("T", bound="GpuTransferStatus")


@_attrs_define
class GpuTransferStatus:
    """Successful GPU mappings consumed by CPU publications or LED decoding.
    Bytes count the mapped payload consumed, excluding RGBA row padding.
    Counters are process totals and reset when the daemon restarts.

        Attributes:
            display_readback_bytes (int):
            display_readback_count (int):
            led_readback_bytes (int):
            led_readback_count (int): Packed LED colors decoded on the CPU, four bytes per LED.
            preview_readback_bytes (int):
            preview_readback_count (int):
            sampling_canvas_readback_bytes (int):
            sampling_canvas_readback_count (int): Full RGBA canvases consumed by CPU sampling or scene/display assembly.
    """

    display_readback_bytes: int
    display_readback_count: int
    led_readback_bytes: int
    led_readback_count: int
    preview_readback_bytes: int
    preview_readback_count: int
    sampling_canvas_readback_bytes: int
    sampling_canvas_readback_count: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        display_readback_bytes = self.display_readback_bytes

        display_readback_count = self.display_readback_count

        led_readback_bytes = self.led_readback_bytes

        led_readback_count = self.led_readback_count

        preview_readback_bytes = self.preview_readback_bytes

        preview_readback_count = self.preview_readback_count

        sampling_canvas_readback_bytes = self.sampling_canvas_readback_bytes

        sampling_canvas_readback_count = self.sampling_canvas_readback_count

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "display_readback_bytes": display_readback_bytes,
                "display_readback_count": display_readback_count,
                "led_readback_bytes": led_readback_bytes,
                "led_readback_count": led_readback_count,
                "preview_readback_bytes": preview_readback_bytes,
                "preview_readback_count": preview_readback_count,
                "sampling_canvas_readback_bytes": sampling_canvas_readback_bytes,
                "sampling_canvas_readback_count": sampling_canvas_readback_count,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        display_readback_bytes = d.pop("display_readback_bytes")

        display_readback_count = d.pop("display_readback_count")

        led_readback_bytes = d.pop("led_readback_bytes")

        led_readback_count = d.pop("led_readback_count")

        preview_readback_bytes = d.pop("preview_readback_bytes")

        preview_readback_count = d.pop("preview_readback_count")

        sampling_canvas_readback_bytes = d.pop("sampling_canvas_readback_bytes")

        sampling_canvas_readback_count = d.pop("sampling_canvas_readback_count")

        gpu_transfer_status = cls(
            display_readback_bytes=display_readback_bytes,
            display_readback_count=display_readback_count,
            led_readback_bytes=led_readback_bytes,
            led_readback_count=led_readback_count,
            preview_readback_bytes=preview_readback_bytes,
            preview_readback_count=preview_readback_count,
            sampling_canvas_readback_bytes=sampling_canvas_readback_bytes,
            sampling_canvas_readback_count=sampling_canvas_readback_count,
        )

        gpu_transfer_status.additional_properties = d
        return gpu_transfer_status

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
