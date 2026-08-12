from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.macos_architecture_api import MacosArchitectureApi

T = TypeVar("T", bound="MacosTahoeCapabilitiesApiStatus")


@_attrs_define
class MacosTahoeCapabilitiesApiStatus:
    """
    Attributes:
        content_tone_mapping_info (bool):
        host_architecture (MacosArchitectureApi):
        metal4 (bool):
        translated_process (bool):
    """

    content_tone_mapping_info: bool
    host_architecture: MacosArchitectureApi
    metal4: bool
    translated_process: bool
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        content_tone_mapping_info = self.content_tone_mapping_info

        host_architecture = self.host_architecture.value

        metal4 = self.metal4

        translated_process = self.translated_process

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "content_tone_mapping_info": content_tone_mapping_info,
                "host_architecture": host_architecture,
                "metal4": metal4,
                "translated_process": translated_process,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        content_tone_mapping_info = d.pop("content_tone_mapping_info")

        host_architecture = MacosArchitectureApi(d.pop("host_architecture"))

        metal4 = d.pop("metal4")

        translated_process = d.pop("translated_process")

        macos_tahoe_capabilities_api_status = cls(
            content_tone_mapping_info=content_tone_mapping_info,
            host_architecture=host_architecture,
            metal4=metal4,
            translated_process=translated_process,
        )

        macos_tahoe_capabilities_api_status.additional_properties = d
        return macos_tahoe_capabilities_api_status

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
