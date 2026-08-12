from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

T = TypeVar("T", bound="MacosTahoeSelectionCapabilitiesApiStatus")


@_attrs_define
class MacosTahoeSelectionCapabilitiesApiStatus:
    """
    Attributes:
        capture_session_generation (int):
        dual_range_screenshots (bool):
        hdr_capture (bool):
        source_id (str):
    """

    capture_session_generation: int
    dual_range_screenshots: bool
    hdr_capture: bool
    source_id: str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        capture_session_generation = self.capture_session_generation

        dual_range_screenshots = self.dual_range_screenshots

        hdr_capture = self.hdr_capture

        source_id = self.source_id

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "capture_session_generation": capture_session_generation,
                "dual_range_screenshots": dual_range_screenshots,
                "hdr_capture": hdr_capture,
                "source_id": source_id,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        capture_session_generation = d.pop("capture_session_generation")

        dual_range_screenshots = d.pop("dual_range_screenshots")

        hdr_capture = d.pop("hdr_capture")

        source_id = d.pop("source_id")

        macos_tahoe_selection_capabilities_api_status = cls(
            capture_session_generation=capture_session_generation,
            dual_range_screenshots=dual_range_screenshots,
            hdr_capture=hdr_capture,
            source_id=source_id,
        )

        macos_tahoe_selection_capabilities_api_status.additional_properties = d
        return macos_tahoe_selection_capabilities_api_status

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
