from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.input_source_platform_status_type_1_type import (
    InputSourcePlatformStatusType1Type,
)
from ..models.macos_authorization_state_api import MacosAuthorizationStateApi
from ..models.macos_capability_owner_api import MacosCapabilityOwnerApi
from ..models.macos_protected_source_state_api import MacosProtectedSourceStateApi
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.macos_daemon_owner_conflict_api_status import (
        MacosDaemonOwnerConflictApiStatus,
    )
    from ..models.macos_screen_telemetry_api_status import MacosScreenTelemetryApiStatus
    from ..models.macos_selection_state_api_type_0 import MacosSelectionStateApiType0
    from ..models.macos_selection_state_api_type_1 import MacosSelectionStateApiType1
    from ..models.macos_selection_state_api_type_2 import MacosSelectionStateApiType2
    from ..models.macos_tahoe_capabilities_api_status import (
        MacosTahoeCapabilitiesApiStatus,
    )
    from ..models.macos_tahoe_selection_capabilities_api_status import (
        MacosTahoeSelectionCapabilitiesApiStatus,
    )


T = TypeVar("T", bound="InputSourcePlatformStatusType1")


@_attrs_define
class InputSourcePlatformStatusType1:
    """
    Attributes:
        owner (MacosCapabilityOwnerApi):
        selection (MacosSelectionStateApiType0 | MacosSelectionStateApiType1 | MacosSelectionStateApiType2):
        state (MacosProtectedSourceStateApi):
        tahoe (MacosTahoeCapabilitiesApiStatus):
        tcc (MacosAuthorizationStateApi):
        telemetry (MacosScreenTelemetryApiStatus):
        type_ (InputSourcePlatformStatusType1Type):
        owner_conflict (MacosDaemonOwnerConflictApiStatus | None | Unset):
        tahoe_selection (MacosTahoeSelectionCapabilitiesApiStatus | None | Unset):
    """

    owner: MacosCapabilityOwnerApi
    selection: (
        MacosSelectionStateApiType0
        | MacosSelectionStateApiType1
        | MacosSelectionStateApiType2
    )
    state: MacosProtectedSourceStateApi
    tahoe: MacosTahoeCapabilitiesApiStatus
    tcc: MacosAuthorizationStateApi
    telemetry: MacosScreenTelemetryApiStatus
    type_: InputSourcePlatformStatusType1Type
    owner_conflict: MacosDaemonOwnerConflictApiStatus | None | Unset = UNSET
    tahoe_selection: MacosTahoeSelectionCapabilitiesApiStatus | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.macos_daemon_owner_conflict_api_status import (
            MacosDaemonOwnerConflictApiStatus,
        )
        from ..models.macos_selection_state_api_type_0 import (
            MacosSelectionStateApiType0,
        )
        from ..models.macos_selection_state_api_type_1 import (
            MacosSelectionStateApiType1,
        )
        from ..models.macos_tahoe_selection_capabilities_api_status import (
            MacosTahoeSelectionCapabilitiesApiStatus,
        )

        owner = self.owner.value

        selection: dict[str, Any]
        if isinstance(self.selection, MacosSelectionStateApiType0):
            selection = self.selection.to_dict()
        elif isinstance(self.selection, MacosSelectionStateApiType1):
            selection = self.selection.to_dict()
        else:
            selection = self.selection.to_dict()

        state = self.state.value

        tahoe = self.tahoe.to_dict()

        tcc = self.tcc.value

        telemetry = self.telemetry.to_dict()

        type_ = self.type_.value

        owner_conflict: dict[str, Any] | None | Unset
        if isinstance(self.owner_conflict, Unset):
            owner_conflict = UNSET
        elif isinstance(self.owner_conflict, MacosDaemonOwnerConflictApiStatus):
            owner_conflict = self.owner_conflict.to_dict()
        else:
            owner_conflict = self.owner_conflict

        tahoe_selection: dict[str, Any] | None | Unset
        if isinstance(self.tahoe_selection, Unset):
            tahoe_selection = UNSET
        elif isinstance(self.tahoe_selection, MacosTahoeSelectionCapabilitiesApiStatus):
            tahoe_selection = self.tahoe_selection.to_dict()
        else:
            tahoe_selection = self.tahoe_selection

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "owner": owner,
                "selection": selection,
                "state": state,
                "tahoe": tahoe,
                "tcc": tcc,
                "telemetry": telemetry,
                "type": type_,
            }
        )
        if owner_conflict is not UNSET:
            field_dict["owner_conflict"] = owner_conflict
        if tahoe_selection is not UNSET:
            field_dict["tahoe_selection"] = tahoe_selection

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.macos_daemon_owner_conflict_api_status import (
            MacosDaemonOwnerConflictApiStatus,
        )
        from ..models.macos_screen_telemetry_api_status import (
            MacosScreenTelemetryApiStatus,
        )
        from ..models.macos_selection_state_api_type_0 import (
            MacosSelectionStateApiType0,
        )
        from ..models.macos_selection_state_api_type_1 import (
            MacosSelectionStateApiType1,
        )
        from ..models.macos_selection_state_api_type_2 import (
            MacosSelectionStateApiType2,
        )
        from ..models.macos_tahoe_capabilities_api_status import (
            MacosTahoeCapabilitiesApiStatus,
        )
        from ..models.macos_tahoe_selection_capabilities_api_status import (
            MacosTahoeSelectionCapabilitiesApiStatus,
        )

        d = dict(src_dict)
        owner = MacosCapabilityOwnerApi(d.pop("owner"))

        def _parse_selection(
            data: object,
        ) -> (
            MacosSelectionStateApiType0
            | MacosSelectionStateApiType1
            | MacosSelectionStateApiType2
        ):
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_macos_selection_state_api_type_0 = (
                    MacosSelectionStateApiType0.from_dict(data)
                )

                return componentsschemas_macos_selection_state_api_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_macos_selection_state_api_type_1 = (
                    MacosSelectionStateApiType1.from_dict(data)
                )

                return componentsschemas_macos_selection_state_api_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            if not isinstance(data, dict):
                raise TypeError()
            componentsschemas_macos_selection_state_api_type_2 = (
                MacosSelectionStateApiType2.from_dict(data)
            )

            return componentsschemas_macos_selection_state_api_type_2

        selection = _parse_selection(d.pop("selection"))

        state = MacosProtectedSourceStateApi(d.pop("state"))

        tahoe = MacosTahoeCapabilitiesApiStatus.from_dict(d.pop("tahoe"))

        tcc = MacosAuthorizationStateApi(d.pop("tcc"))

        telemetry = MacosScreenTelemetryApiStatus.from_dict(d.pop("telemetry"))

        type_ = InputSourcePlatformStatusType1Type(d.pop("type"))

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

        def _parse_tahoe_selection(
            data: object,
        ) -> MacosTahoeSelectionCapabilitiesApiStatus | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                tahoe_selection_type_1 = (
                    MacosTahoeSelectionCapabilitiesApiStatus.from_dict(data)
                )

                return tahoe_selection_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(MacosTahoeSelectionCapabilitiesApiStatus | None | Unset, data)

        tahoe_selection = _parse_tahoe_selection(d.pop("tahoe_selection", UNSET))

        input_source_platform_status_type_1 = cls(
            owner=owner,
            selection=selection,
            state=state,
            tahoe=tahoe,
            tcc=tcc,
            telemetry=telemetry,
            type_=type_,
            owner_conflict=owner_conflict,
            tahoe_selection=tahoe_selection,
        )

        input_source_platform_status_type_1.additional_properties = d
        return input_source_platform_status_type_1

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
