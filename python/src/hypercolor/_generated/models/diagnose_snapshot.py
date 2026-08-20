from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.diagnose_device_output_snapshot import DiagnoseDeviceOutputSnapshot
    from ..models.diagnose_display_output_snapshot import DiagnoseDisplayOutputSnapshot
    from ..models.diagnose_render_snapshot import DiagnoseRenderSnapshot
    from ..models.diagnose_usb_actor_snapshot import DiagnoseUsbActorSnapshot
    from ..models.input_status import InputStatus


T = TypeVar("T", bound="DiagnoseSnapshot")


@_attrs_define
class DiagnoseSnapshot:
    """
    Attributes:
        device_output (DiagnoseDeviceOutputSnapshot):
        display_output (DiagnoseDisplayOutputSnapshot):
        input_ (InputStatus): Host keyboard/mouse capture health, for consent and remediation UX.

            `enabled` is the consent config gate. `host_capturing` is true when a
            host backend is actively reading device nodes. `devices_denied` counts
            input nodes present but unreadable (udev rules missing) — the signal
            that distinguishes "input is off" from "input is on but blocked".

            `degraded` carries the failures the counters cannot express. Windows has no
            per-device denial to count: either the process has a visible window station
            and sees input, or it does not, and that is a session-level fact rather than
            a per-node one.
        render (DiagnoseRenderSnapshot):
        usb (DiagnoseUsbActorSnapshot):
        macos_screen_parity (Any | Unset):
    """

    device_output: DiagnoseDeviceOutputSnapshot
    display_output: DiagnoseDisplayOutputSnapshot
    input_: InputStatus
    render: DiagnoseRenderSnapshot
    usb: DiagnoseUsbActorSnapshot
    macos_screen_parity: Any | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        device_output = self.device_output.to_dict()

        display_output = self.display_output.to_dict()

        input_ = self.input_.to_dict()

        render = self.render.to_dict()

        usb = self.usb.to_dict()

        macos_screen_parity = self.macos_screen_parity

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "device_output": device_output,
                "display_output": display_output,
                "input": input_,
                "render": render,
                "usb": usb,
            }
        )
        if macos_screen_parity is not UNSET:
            field_dict["macos_screen_parity"] = macos_screen_parity

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.diagnose_device_output_snapshot import (
            DiagnoseDeviceOutputSnapshot,
        )
        from ..models.diagnose_display_output_snapshot import (
            DiagnoseDisplayOutputSnapshot,
        )
        from ..models.diagnose_render_snapshot import DiagnoseRenderSnapshot
        from ..models.diagnose_usb_actor_snapshot import DiagnoseUsbActorSnapshot
        from ..models.input_status import InputStatus

        d = dict(src_dict)
        device_output = DiagnoseDeviceOutputSnapshot.from_dict(d.pop("device_output"))

        display_output = DiagnoseDisplayOutputSnapshot.from_dict(
            d.pop("display_output")
        )

        input_ = InputStatus.from_dict(d.pop("input"))

        render = DiagnoseRenderSnapshot.from_dict(d.pop("render"))

        usb = DiagnoseUsbActorSnapshot.from_dict(d.pop("usb"))

        macos_screen_parity = d.pop("macos_screen_parity", UNSET)

        diagnose_snapshot = cls(
            device_output=device_output,
            display_output=display_output,
            input_=input_,
            render=render,
            usb=usb,
            macos_screen_parity=macos_screen_parity,
        )

        diagnose_snapshot.additional_properties = d
        return diagnose_snapshot

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
