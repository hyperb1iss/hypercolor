from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.device_summary import DeviceSummary


T = TypeVar("T", bound="PairDeviceResponse")


@_attrs_define
class PairDeviceResponse:
    """Response for `POST /api/v1/devices/{id}/pair`.

    `device` carries the device's refreshed summary when pairing changed
    its state enough to be worth re-rendering, and is omitted otherwise.

        Attributes:
            message (str):
            status (str):
            activated (bool | Unset): Whether the device was connected and started rendering as part of
                the pairing.
            device (DeviceSummary | None | Unset):
    """

    message: str
    status: str
    activated: bool | Unset = UNSET
    device: DeviceSummary | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.device_summary import DeviceSummary

        message = self.message

        status = self.status

        activated = self.activated

        device: dict[str, Any] | None | Unset
        if isinstance(self.device, Unset):
            device = UNSET
        elif isinstance(self.device, DeviceSummary):
            device = self.device.to_dict()
        else:
            device = self.device

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "message": message,
                "status": status,
            }
        )
        if activated is not UNSET:
            field_dict["activated"] = activated
        if device is not UNSET:
            field_dict["device"] = device

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.device_summary import DeviceSummary

        d = dict(src_dict)
        message = d.pop("message")

        status = d.pop("status")

        activated = d.pop("activated", UNSET)

        def _parse_device(data: object) -> DeviceSummary | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                device_type_1 = DeviceSummary.from_dict(data)

                return device_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(DeviceSummary | None | Unset, data)

        device = _parse_device(d.pop("device", UNSET))

        pair_device_response = cls(
            message=message,
            status=status,
            activated=activated,
            device=device,
        )

        pair_device_response.additional_properties = d
        return pair_device_response

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
