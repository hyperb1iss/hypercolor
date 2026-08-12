from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.input_source_platform_status_type_0_type import (
    InputSourcePlatformStatusType0Type,
)
from ..models.macos_authorization_state_api import MacosAuthorizationStateApi
from ..models.macos_capability_owner_api import MacosCapabilityOwnerApi
from ..models.macos_protected_source_state_api import MacosProtectedSourceStateApi
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.macos_daemon_owner_conflict_api_status import (
        MacosDaemonOwnerConflictApiStatus,
    )
    from ..models.macos_input_telemetry_api_status import MacosInputTelemetryApiStatus


T = TypeVar("T", bound="InputSourcePlatformStatusType0")


@_attrs_define
class InputSourcePlatformStatusType0:
    """
    Attributes:
        keyboard (MacosProtectedSourceStateApi):
        keyboard_owner (MacosCapabilityOwnerApi):
        keyboard_tcc (MacosAuthorizationStateApi):
        pointer (MacosProtectedSourceStateApi):
        pointer_owner (MacosCapabilityOwnerApi):
        telemetry (MacosInputTelemetryApiStatus):
        type_ (InputSourcePlatformStatusType0Type):
        owner_conflict (MacosDaemonOwnerConflictApiStatus | None | Unset):
    """

    keyboard: MacosProtectedSourceStateApi
    keyboard_owner: MacosCapabilityOwnerApi
    keyboard_tcc: MacosAuthorizationStateApi
    pointer: MacosProtectedSourceStateApi
    pointer_owner: MacosCapabilityOwnerApi
    telemetry: MacosInputTelemetryApiStatus
    type_: InputSourcePlatformStatusType0Type
    owner_conflict: MacosDaemonOwnerConflictApiStatus | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.macos_daemon_owner_conflict_api_status import (
            MacosDaemonOwnerConflictApiStatus,
        )

        keyboard = self.keyboard.value

        keyboard_owner = self.keyboard_owner.value

        keyboard_tcc = self.keyboard_tcc.value

        pointer = self.pointer.value

        pointer_owner = self.pointer_owner.value

        telemetry = self.telemetry.to_dict()

        type_ = self.type_.value

        owner_conflict: dict[str, Any] | None | Unset
        if isinstance(self.owner_conflict, Unset):
            owner_conflict = UNSET
        elif isinstance(self.owner_conflict, MacosDaemonOwnerConflictApiStatus):
            owner_conflict = self.owner_conflict.to_dict()
        else:
            owner_conflict = self.owner_conflict

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "keyboard": keyboard,
                "keyboard_owner": keyboard_owner,
                "keyboard_tcc": keyboard_tcc,
                "pointer": pointer,
                "pointer_owner": pointer_owner,
                "telemetry": telemetry,
                "type": type_,
            }
        )
        if owner_conflict is not UNSET:
            field_dict["owner_conflict"] = owner_conflict

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.macos_daemon_owner_conflict_api_status import (
            MacosDaemonOwnerConflictApiStatus,
        )
        from ..models.macos_input_telemetry_api_status import (
            MacosInputTelemetryApiStatus,
        )

        d = dict(src_dict)
        keyboard = MacosProtectedSourceStateApi(d.pop("keyboard"))

        keyboard_owner = MacosCapabilityOwnerApi(d.pop("keyboard_owner"))

        keyboard_tcc = MacosAuthorizationStateApi(d.pop("keyboard_tcc"))

        pointer = MacosProtectedSourceStateApi(d.pop("pointer"))

        pointer_owner = MacosCapabilityOwnerApi(d.pop("pointer_owner"))

        telemetry = MacosInputTelemetryApiStatus.from_dict(d.pop("telemetry"))

        type_ = InputSourcePlatformStatusType0Type(d.pop("type"))

        def _parse_owner_conflict(
            data: object,
        ) -> MacosDaemonOwnerConflictApiStatus | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                owner_conflict_type_1 = MacosDaemonOwnerConflictApiStatus.from_dict(
                    data
                )

                return owner_conflict_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(MacosDaemonOwnerConflictApiStatus | None | Unset, data)

        owner_conflict = _parse_owner_conflict(d.pop("owner_conflict", UNSET))

        input_source_platform_status_type_0 = cls(
            keyboard=keyboard,
            keyboard_owner=keyboard_owner,
            keyboard_tcc=keyboard_tcc,
            pointer=pointer,
            pointer_owner=pointer_owner,
            telemetry=telemetry,
            type_=type_,
            owner_conflict=owner_conflict,
        )

        input_source_platform_status_type_0.additional_properties = d
        return input_source_platform_status_type_0

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
