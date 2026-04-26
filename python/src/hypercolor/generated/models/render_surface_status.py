from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

T = TypeVar("T", bound="RenderSurfaceStatus")


@_attrs_define
class RenderSurfaceStatus:
    """
    Attributes:
        canvas_receivers (int):
        dequeued_slots (int):
        free_slots (int):
        published_slots (int):
        slot_count (int):
    """

    canvas_receivers: int
    dequeued_slots: int
    free_slots: int
    published_slots: int
    slot_count: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        canvas_receivers = self.canvas_receivers

        dequeued_slots = self.dequeued_slots

        free_slots = self.free_slots

        published_slots = self.published_slots

        slot_count = self.slot_count

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "canvas_receivers": canvas_receivers,
                "dequeued_slots": dequeued_slots,
                "free_slots": free_slots,
                "published_slots": published_slots,
                "slot_count": slot_count,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        canvas_receivers = d.pop("canvas_receivers")

        dequeued_slots = d.pop("dequeued_slots")

        free_slots = d.pop("free_slots")

        published_slots = d.pop("published_slots")

        slot_count = d.pop("slot_count")

        render_surface_status = cls(
            canvas_receivers=canvas_receivers,
            dequeued_slots=dequeued_slots,
            free_slots=free_slots,
            published_slots=published_slots,
            slot_count=slot_count,
        )

        render_surface_status.additional_properties = d
        return render_surface_status

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
