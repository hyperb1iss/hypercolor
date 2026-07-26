from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

T = TypeVar("T", bound="InputStatus")


@_attrs_define
class InputStatus:
    """Host keyboard/mouse capture health, for consent and remediation UX.

    `enabled` is the consent config gate. `host_capturing` is true when a
    host backend is actively reading device nodes. `devices_denied` counts
    input nodes present but unreadable (udev rules missing) — the signal
    that distinguishes "input is off" from "input is on but blocked".

        Attributes:
            backends (list[str]):
            devices_denied (int):
            devices_opened (int):
            enabled (bool):
            host_capture_registered (bool):
            host_capturing (bool):
    """

    backends: list[str]
    devices_denied: int
    devices_opened: int
    enabled: bool
    host_capture_registered: bool
    host_capturing: bool
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        backends = self.backends

        devices_denied = self.devices_denied

        devices_opened = self.devices_opened

        enabled = self.enabled

        host_capture_registered = self.host_capture_registered

        host_capturing = self.host_capturing

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "backends": backends,
                "devices_denied": devices_denied,
                "devices_opened": devices_opened,
                "enabled": enabled,
                "host_capture_registered": host_capture_registered,
                "host_capturing": host_capturing,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        backends = cast(list[str], d.pop("backends"))

        devices_denied = d.pop("devices_denied")

        devices_opened = d.pop("devices_opened")

        enabled = d.pop("enabled")

        host_capture_registered = d.pop("host_capture_registered")

        host_capturing = d.pop("host_capturing")

        input_status = cls(
            backends=backends,
            devices_denied=devices_denied,
            devices_opened=devices_opened,
            enabled=enabled,
            host_capture_registered=host_capture_registered,
            host_capturing=host_capturing,
        )

        input_status.additional_properties = d
        return input_status

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
