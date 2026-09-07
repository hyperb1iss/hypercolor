from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.diagnose_device_output_item import DiagnoseDeviceOutputItem


T = TypeVar("T", bound="DiagnoseDeviceOutputSnapshot")


@_attrs_define
class DiagnoseDeviceOutputSnapshot:
    """
    Attributes:
        dropped_frames_total (int):
        errors_total (int):
        items (list[DiagnoseDeviceOutputItem]):
        lagging_queues (int):
        queues (int):
        usb_queues (int):
        worker_finished_queues (int):
    """

    dropped_frames_total: int
    errors_total: int
    items: list[DiagnoseDeviceOutputItem]
    lagging_queues: int
    queues: int
    usb_queues: int
    worker_finished_queues: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        dropped_frames_total = self.dropped_frames_total

        errors_total = self.errors_total

        items = []
        for items_item_data in self.items:
            items_item = items_item_data.to_dict()
            items.append(items_item)

        lagging_queues = self.lagging_queues

        queues = self.queues

        usb_queues = self.usb_queues

        worker_finished_queues = self.worker_finished_queues

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "dropped_frames_total": dropped_frames_total,
                "errors_total": errors_total,
                "items": items,
                "lagging_queues": lagging_queues,
                "queues": queues,
                "usb_queues": usb_queues,
                "worker_finished_queues": worker_finished_queues,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.diagnose_device_output_item import DiagnoseDeviceOutputItem

        d = dict(src_dict)
        dropped_frames_total = d.pop("dropped_frames_total")

        errors_total = d.pop("errors_total")

        items = []
        _items = d.pop("items")
        for items_item_data in _items:
            items_item = DiagnoseDeviceOutputItem.from_dict(items_item_data)

            items.append(items_item)

        lagging_queues = d.pop("lagging_queues")

        queues = d.pop("queues")

        usb_queues = d.pop("usb_queues")

        worker_finished_queues = d.pop("worker_finished_queues")

        diagnose_device_output_snapshot = cls(
            dropped_frames_total=dropped_frames_total,
            errors_total=errors_total,
            items=items,
            lagging_queues=lagging_queues,
            queues=queues,
            usb_queues=usb_queues,
            worker_finished_queues=worker_finished_queues,
        )

        diagnose_device_output_snapshot.additional_properties = d
        return diagnose_device_output_snapshot

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
