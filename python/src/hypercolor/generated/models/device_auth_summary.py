from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.device_auth_state import DeviceAuthState
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.pairing_descriptor import PairingDescriptor


T = TypeVar("T", bound="DeviceAuthSummary")


@_attrs_define
class DeviceAuthSummary:
    """Driver-owned authentication summary for one tracked device.

    Attributes:
        can_pair (bool):
        state (DeviceAuthState): Summary of whether a device needs authentication before it can be used.
        descriptor (None | PairingDescriptor | Unset):
        last_error (None | str | Unset):
    """

    can_pair: bool
    state: DeviceAuthState
    descriptor: None | PairingDescriptor | Unset = UNSET
    last_error: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.pairing_descriptor import PairingDescriptor

        can_pair = self.can_pair

        state = self.state.value

        descriptor: dict[str, Any] | None | Unset
        if isinstance(self.descriptor, Unset):
            descriptor = UNSET
        elif isinstance(self.descriptor, PairingDescriptor):
            descriptor = self.descriptor.to_dict()
        else:
            descriptor = self.descriptor

        last_error: None | str | Unset
        if isinstance(self.last_error, Unset):
            last_error = UNSET
        else:
            last_error = self.last_error

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "can_pair": can_pair,
                "state": state,
            }
        )
        if descriptor is not UNSET:
            field_dict["descriptor"] = descriptor
        if last_error is not UNSET:
            field_dict["last_error"] = last_error

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.pairing_descriptor import PairingDescriptor

        d = dict(src_dict)
        can_pair = d.pop("can_pair")

        state = DeviceAuthState(d.pop("state"))

        def _parse_descriptor(data: object) -> None | PairingDescriptor | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                descriptor_type_1 = PairingDescriptor.from_dict(data)

                return descriptor_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | PairingDescriptor | Unset, data)

        descriptor = _parse_descriptor(d.pop("descriptor", UNSET))

        def _parse_last_error(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        last_error = _parse_last_error(d.pop("last_error", UNSET))

        device_auth_summary = cls(
            can_pair=can_pair,
            state=state,
            descriptor=descriptor,
            last_error=last_error,
        )

        device_auth_summary.additional_properties = d
        return device_auth_summary

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
