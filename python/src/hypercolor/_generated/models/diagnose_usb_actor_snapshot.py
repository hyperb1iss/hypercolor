from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

T = TypeVar("T", bound="DiagnoseUsbActorSnapshot")


@_attrs_define
class DiagnoseUsbActorSnapshot:
    """
    Attributes:
        display_frames_delayed_for_led_total (int):
        display_frames_total (int):
        display_led_priority_wait_avg_ms (float):
        display_led_priority_wait_max_ms (float):
        display_led_priority_wait_total_ms (float):
    """

    display_frames_delayed_for_led_total: int
    display_frames_total: int
    display_led_priority_wait_avg_ms: float
    display_led_priority_wait_max_ms: float
    display_led_priority_wait_total_ms: float
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        display_frames_delayed_for_led_total = self.display_frames_delayed_for_led_total

        display_frames_total = self.display_frames_total

        display_led_priority_wait_avg_ms = self.display_led_priority_wait_avg_ms

        display_led_priority_wait_max_ms = self.display_led_priority_wait_max_ms

        display_led_priority_wait_total_ms = self.display_led_priority_wait_total_ms

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "display_frames_delayed_for_led_total": display_frames_delayed_for_led_total,
                "display_frames_total": display_frames_total,
                "display_led_priority_wait_avg_ms": display_led_priority_wait_avg_ms,
                "display_led_priority_wait_max_ms": display_led_priority_wait_max_ms,
                "display_led_priority_wait_total_ms": display_led_priority_wait_total_ms,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        display_frames_delayed_for_led_total = d.pop(
            "display_frames_delayed_for_led_total"
        )

        display_frames_total = d.pop("display_frames_total")

        display_led_priority_wait_avg_ms = d.pop("display_led_priority_wait_avg_ms")

        display_led_priority_wait_max_ms = d.pop("display_led_priority_wait_max_ms")

        display_led_priority_wait_total_ms = d.pop("display_led_priority_wait_total_ms")

        diagnose_usb_actor_snapshot = cls(
            display_frames_delayed_for_led_total=display_frames_delayed_for_led_total,
            display_frames_total=display_frames_total,
            display_led_priority_wait_avg_ms=display_led_priority_wait_avg_ms,
            display_led_priority_wait_max_ms=display_led_priority_wait_max_ms,
            display_led_priority_wait_total_ms=display_led_priority_wait_total_ms,
        )

        diagnose_usb_actor_snapshot.additional_properties = d
        return diagnose_usb_actor_snapshot

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
