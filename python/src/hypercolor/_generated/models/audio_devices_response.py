from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.audio_device_info import AudioDeviceInfo


T = TypeVar("T", bound="AudioDevicesResponse")


@_attrs_define
class AudioDevicesResponse:
    """The audio input inventory plus the configured selection.

    Attributes:
        current (str):
        devices (list[AudioDeviceInfo]):
    """

    current: str
    devices: list[AudioDeviceInfo]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        current = self.current

        devices = []
        for devices_item_data in self.devices:
            devices_item = devices_item_data.to_dict()
            devices.append(devices_item)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "current": current,
                "devices": devices,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.audio_device_info import AudioDeviceInfo

        d = dict(src_dict)
        current = d.pop("current")

        devices = []
        _devices = d.pop("devices")
        for devices_item_data in _devices:
            devices_item = AudioDeviceInfo.from_dict(devices_item_data)

            devices.append(devices_item)

        audio_devices_response = cls(
            current=current,
            devices=devices,
        )

        audio_devices_response.additional_properties = d
        return audio_devices_response

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
