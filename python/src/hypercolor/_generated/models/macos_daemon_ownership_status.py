from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.macos_capability_owner import MacosCapabilityOwner
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.macos_daemon_owner_conflict_status import (
        MacosDaemonOwnerConflictStatus,
    )
    from ..models.macos_daemon_owner_recovery_required_status import (
        MacosDaemonOwnerRecoveryRequiredStatus,
    )


T = TypeVar("T", bound="MacosDaemonOwnershipStatus")


@_attrs_define
class MacosDaemonOwnershipStatus:
    """
    Attributes:
        active_owner (MacosCapabilityOwner):
        owner_epoch (int):
        conflict (MacosDaemonOwnerConflictStatus | None | Unset):
        recovery_required (MacosDaemonOwnerRecoveryRequiredStatus | None | Unset):
    """

    active_owner: MacosCapabilityOwner
    owner_epoch: int
    conflict: MacosDaemonOwnerConflictStatus | None | Unset = UNSET
    recovery_required: MacosDaemonOwnerRecoveryRequiredStatus | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.macos_daemon_owner_conflict_status import (
            MacosDaemonOwnerConflictStatus,
        )
        from ..models.macos_daemon_owner_recovery_required_status import (
            MacosDaemonOwnerRecoveryRequiredStatus,
        )

        active_owner = self.active_owner.value

        owner_epoch = self.owner_epoch

        conflict: dict[str, Any] | None | Unset
        if isinstance(self.conflict, Unset):
            conflict = UNSET
        elif isinstance(self.conflict, MacosDaemonOwnerConflictStatus):
            conflict = self.conflict.to_dict()
        else:
            conflict = self.conflict

        recovery_required: dict[str, Any] | None | Unset
        if isinstance(self.recovery_required, Unset):
            recovery_required = UNSET
        elif isinstance(self.recovery_required, MacosDaemonOwnerRecoveryRequiredStatus):
            recovery_required = self.recovery_required.to_dict()
        else:
            recovery_required = self.recovery_required

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "active_owner": active_owner,
                "owner_epoch": owner_epoch,
            }
        )
        if conflict is not UNSET:
            field_dict["conflict"] = conflict
        if recovery_required is not UNSET:
            field_dict["recovery_required"] = recovery_required

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.macos_daemon_owner_conflict_status import (
            MacosDaemonOwnerConflictStatus,
        )
        from ..models.macos_daemon_owner_recovery_required_status import (
            MacosDaemonOwnerRecoveryRequiredStatus,
        )

        d = dict(src_dict)
        active_owner = MacosCapabilityOwner(d.pop("active_owner"))

        owner_epoch = d.pop("owner_epoch")

        def _parse_conflict(
            data: object,
        ) -> MacosDaemonOwnerConflictStatus | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                conflict_type_1 = MacosDaemonOwnerConflictStatus.from_dict(data)

                return conflict_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(MacosDaemonOwnerConflictStatus | None | Unset, data)

        conflict = _parse_conflict(d.pop("conflict", UNSET))

        def _parse_recovery_required(
            data: object,
        ) -> MacosDaemonOwnerRecoveryRequiredStatus | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                recovery_required_type_1 = (
                    MacosDaemonOwnerRecoveryRequiredStatus.from_dict(data)
                )

                return recovery_required_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(MacosDaemonOwnerRecoveryRequiredStatus | None | Unset, data)

        recovery_required = _parse_recovery_required(d.pop("recovery_required", UNSET))

        macos_daemon_ownership_status = cls(
            active_owner=active_owner,
            owner_epoch=owner_epoch,
            conflict=conflict,
            recovery_required=recovery_required,
        )

        macos_daemon_ownership_status.additional_properties = d
        return macos_daemon_ownership_status

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
