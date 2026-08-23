from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.device_summary import DeviceSummary


T = TypeVar("T", bound="DeletePairingResponse")


@_attrs_define
class DeletePairingResponse:
    """Response for `DELETE /api/v1/devices/{id}/pair`.

    Attributes:
        message (str):
        device (DeviceSummary | None | Unset):
        disconnected (bool | Unset): Whether forgetting the credentials also dropped a live connection.
        status (str | Unset):
    """

    message: str
    device: DeviceSummary | None | Unset = UNSET
    disconnected: bool | Unset = UNSET
    status: str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.device_summary import DeviceSummary

        message = self.message

        device: dict[str, Any] | None | Unset
        if isinstance(self.device, Unset):
            device = UNSET
        elif isinstance(self.device, DeviceSummary):
            device = self.device.to_dict()
        else:
            device = self.device

        disconnected = self.disconnected

        status = self.status

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "message": message,
            }
        )
        if device is not UNSET:
            field_dict["device"] = device
        if disconnected is not UNSET:
            field_dict["disconnected"] = disconnected
        if status is not UNSET:
            field_dict["status"] = status

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.device_summary import DeviceSummary

        d = dict(src_dict)
        message = d.pop("message")

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

        disconnected = d.pop("disconnected", UNSET)

        status = d.pop("status", UNSET)

        delete_pairing_response = cls(
            message=message,
            device=device,
            disconnected=disconnected,
            status=status,
        )

        delete_pairing_response.additional_properties = d
        return delete_pairing_response

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
