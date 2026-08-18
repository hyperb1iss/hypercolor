from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.macos_timing_api_status import MacosTimingApiStatus


T = TypeVar("T", bound="MacosScreenTimingApiStatus")


@_attrs_define
class MacosScreenTimingApiStatus:
    """
    Attributes:
        callback (MacosTimingApiStatus):
        capture_to_converted_publication (MacosTimingApiStatus):
        capture_to_native_publication (MacosTimingApiStatus):
        conversion (MacosTimingApiStatus):
        cpu_reduction (MacosTimingApiStatus):
        enqueue (MacosTimingApiStatus):
        native_import (MacosTimingApiStatus):
        native_reduction_submit (MacosTimingApiStatus):
        publication (MacosTimingApiStatus):
        retain (MacosTimingApiStatus):
    """

    callback: MacosTimingApiStatus
    capture_to_converted_publication: MacosTimingApiStatus
    capture_to_native_publication: MacosTimingApiStatus
    conversion: MacosTimingApiStatus
    cpu_reduction: MacosTimingApiStatus
    enqueue: MacosTimingApiStatus
    native_import: MacosTimingApiStatus
    native_reduction_submit: MacosTimingApiStatus
    publication: MacosTimingApiStatus
    retain: MacosTimingApiStatus
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        callback = self.callback.to_dict()

        capture_to_converted_publication = (
            self.capture_to_converted_publication.to_dict()
        )

        capture_to_native_publication = self.capture_to_native_publication.to_dict()

        conversion = self.conversion.to_dict()

        cpu_reduction = self.cpu_reduction.to_dict()

        enqueue = self.enqueue.to_dict()

        native_import = self.native_import.to_dict()

        native_reduction_submit = self.native_reduction_submit.to_dict()

        publication = self.publication.to_dict()

        retain = self.retain.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "callback": callback,
                "capture_to_converted_publication": capture_to_converted_publication,
                "capture_to_native_publication": capture_to_native_publication,
                "conversion": conversion,
                "cpu_reduction": cpu_reduction,
                "enqueue": enqueue,
                "native_import": native_import,
                "native_reduction_submit": native_reduction_submit,
                "publication": publication,
                "retain": retain,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.macos_timing_api_status import MacosTimingApiStatus

        d = dict(src_dict)
        callback = MacosTimingApiStatus.from_dict(d.pop("callback"))

        capture_to_converted_publication = MacosTimingApiStatus.from_dict(
            d.pop("capture_to_converted_publication")
        )

        capture_to_native_publication = MacosTimingApiStatus.from_dict(
            d.pop("capture_to_native_publication")
        )

        conversion = MacosTimingApiStatus.from_dict(d.pop("conversion"))

        cpu_reduction = MacosTimingApiStatus.from_dict(d.pop("cpu_reduction"))

        enqueue = MacosTimingApiStatus.from_dict(d.pop("enqueue"))

        native_import = MacosTimingApiStatus.from_dict(d.pop("native_import"))

        native_reduction_submit = MacosTimingApiStatus.from_dict(
            d.pop("native_reduction_submit")
        )

        publication = MacosTimingApiStatus.from_dict(d.pop("publication"))

        retain = MacosTimingApiStatus.from_dict(d.pop("retain"))

        macos_screen_timing_api_status = cls(
            callback=callback,
            capture_to_converted_publication=capture_to_converted_publication,
            capture_to_native_publication=capture_to_native_publication,
            conversion=conversion,
            cpu_reduction=cpu_reduction,
            enqueue=enqueue,
            native_import=native_import,
            native_reduction_submit=native_reduction_submit,
            publication=publication,
            retain=retain,
        )

        macos_screen_timing_api_status.additional_properties = d
        return macos_screen_timing_api_status

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
