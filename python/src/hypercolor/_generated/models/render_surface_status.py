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
        compositor_pool_dequeued_slots (int):
        compositor_pool_free_slots (int):
        compositor_pool_published_slots (int):
        compositor_pool_slot_count (int):
        dequeued_slots (int): Deprecated v1 alias for `scene_pool_dequeued_slots`.
        direct_pool_dequeued_slots (int):
        direct_pool_free_slots (int):
        direct_pool_published_slots (int):
        direct_pool_slot_count (int):
        free_slots (int): Deprecated v1 alias for `scene_pool_free_slots`.
        preview_pool_dequeued_slots (int):
        preview_pool_free_slots (int):
        preview_pool_published_slots (int):
        preview_pool_slot_count (int):
        published_slots (int): Deprecated v1 alias for `scene_pool_published_slots`.
        scene_pool_dequeued_slots (int):
        scene_pool_free_slots (int):
        scene_pool_published_slots (int):
        scene_pool_slot_count (int):
        slot_count (int): Deprecated v1 alias for `scene_pool_slot_count`.
    """

    canvas_receivers: int
    compositor_pool_dequeued_slots: int
    compositor_pool_free_slots: int
    compositor_pool_published_slots: int
    compositor_pool_slot_count: int
    dequeued_slots: int
    direct_pool_dequeued_slots: int
    direct_pool_free_slots: int
    direct_pool_published_slots: int
    direct_pool_slot_count: int
    free_slots: int
    preview_pool_dequeued_slots: int
    preview_pool_free_slots: int
    preview_pool_published_slots: int
    preview_pool_slot_count: int
    published_slots: int
    scene_pool_dequeued_slots: int
    scene_pool_free_slots: int
    scene_pool_published_slots: int
    scene_pool_slot_count: int
    slot_count: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        canvas_receivers = self.canvas_receivers

        compositor_pool_dequeued_slots = self.compositor_pool_dequeued_slots

        compositor_pool_free_slots = self.compositor_pool_free_slots

        compositor_pool_published_slots = self.compositor_pool_published_slots

        compositor_pool_slot_count = self.compositor_pool_slot_count

        dequeued_slots = self.dequeued_slots

        direct_pool_dequeued_slots = self.direct_pool_dequeued_slots

        direct_pool_free_slots = self.direct_pool_free_slots

        direct_pool_published_slots = self.direct_pool_published_slots

        direct_pool_slot_count = self.direct_pool_slot_count

        free_slots = self.free_slots

        preview_pool_dequeued_slots = self.preview_pool_dequeued_slots

        preview_pool_free_slots = self.preview_pool_free_slots

        preview_pool_published_slots = self.preview_pool_published_slots

        preview_pool_slot_count = self.preview_pool_slot_count

        published_slots = self.published_slots

        scene_pool_dequeued_slots = self.scene_pool_dequeued_slots

        scene_pool_free_slots = self.scene_pool_free_slots

        scene_pool_published_slots = self.scene_pool_published_slots

        scene_pool_slot_count = self.scene_pool_slot_count

        slot_count = self.slot_count

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "canvas_receivers": canvas_receivers,
                "compositor_pool_dequeued_slots": compositor_pool_dequeued_slots,
                "compositor_pool_free_slots": compositor_pool_free_slots,
                "compositor_pool_published_slots": compositor_pool_published_slots,
                "compositor_pool_slot_count": compositor_pool_slot_count,
                "dequeued_slots": dequeued_slots,
                "direct_pool_dequeued_slots": direct_pool_dequeued_slots,
                "direct_pool_free_slots": direct_pool_free_slots,
                "direct_pool_published_slots": direct_pool_published_slots,
                "direct_pool_slot_count": direct_pool_slot_count,
                "free_slots": free_slots,
                "preview_pool_dequeued_slots": preview_pool_dequeued_slots,
                "preview_pool_free_slots": preview_pool_free_slots,
                "preview_pool_published_slots": preview_pool_published_slots,
                "preview_pool_slot_count": preview_pool_slot_count,
                "published_slots": published_slots,
                "scene_pool_dequeued_slots": scene_pool_dequeued_slots,
                "scene_pool_free_slots": scene_pool_free_slots,
                "scene_pool_published_slots": scene_pool_published_slots,
                "scene_pool_slot_count": scene_pool_slot_count,
                "slot_count": slot_count,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        canvas_receivers = d.pop("canvas_receivers")

        compositor_pool_dequeued_slots = d.pop("compositor_pool_dequeued_slots")

        compositor_pool_free_slots = d.pop("compositor_pool_free_slots")

        compositor_pool_published_slots = d.pop("compositor_pool_published_slots")

        compositor_pool_slot_count = d.pop("compositor_pool_slot_count")

        dequeued_slots = d.pop("dequeued_slots")

        direct_pool_dequeued_slots = d.pop("direct_pool_dequeued_slots")

        direct_pool_free_slots = d.pop("direct_pool_free_slots")

        direct_pool_published_slots = d.pop("direct_pool_published_slots")

        direct_pool_slot_count = d.pop("direct_pool_slot_count")

        free_slots = d.pop("free_slots")

        preview_pool_dequeued_slots = d.pop("preview_pool_dequeued_slots")

        preview_pool_free_slots = d.pop("preview_pool_free_slots")

        preview_pool_published_slots = d.pop("preview_pool_published_slots")

        preview_pool_slot_count = d.pop("preview_pool_slot_count")

        published_slots = d.pop("published_slots")

        scene_pool_dequeued_slots = d.pop("scene_pool_dequeued_slots")

        scene_pool_free_slots = d.pop("scene_pool_free_slots")

        scene_pool_published_slots = d.pop("scene_pool_published_slots")

        scene_pool_slot_count = d.pop("scene_pool_slot_count")

        slot_count = d.pop("slot_count")

        render_surface_status = cls(
            canvas_receivers=canvas_receivers,
            compositor_pool_dequeued_slots=compositor_pool_dequeued_slots,
            compositor_pool_free_slots=compositor_pool_free_slots,
            compositor_pool_published_slots=compositor_pool_published_slots,
            compositor_pool_slot_count=compositor_pool_slot_count,
            dequeued_slots=dequeued_slots,
            direct_pool_dequeued_slots=direct_pool_dequeued_slots,
            direct_pool_free_slots=direct_pool_free_slots,
            direct_pool_published_slots=direct_pool_published_slots,
            direct_pool_slot_count=direct_pool_slot_count,
            free_slots=free_slots,
            preview_pool_dequeued_slots=preview_pool_dequeued_slots,
            preview_pool_free_slots=preview_pool_free_slots,
            preview_pool_published_slots=preview_pool_published_slots,
            preview_pool_slot_count=preview_pool_slot_count,
            published_slots=published_slots,
            scene_pool_dequeued_slots=scene_pool_dequeued_slots,
            scene_pool_free_slots=scene_pool_free_slots,
            scene_pool_published_slots=scene_pool_published_slots,
            scene_pool_slot_count=scene_pool_slot_count,
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
