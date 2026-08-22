from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.macos_capability_owner import MacosCapabilityOwner
from ..models.macos_daemon_handover_phase import MacosDaemonHandoverPhase

T = TypeVar("T", bound="MacosDaemonOwnerRecoveryRequiredStatus")


@_attrs_define
class MacosDaemonOwnerRecoveryRequiredStatus:
    """
    Attributes:
        phase (MacosDaemonHandoverPhase):
        prior_owner (MacosCapabilityOwner):
        requested_owner (MacosCapabilityOwner):
    """

    phase: MacosDaemonHandoverPhase
    prior_owner: MacosCapabilityOwner
    requested_owner: MacosCapabilityOwner
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        phase = self.phase.value

        prior_owner = self.prior_owner.value

        requested_owner = self.requested_owner.value

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "phase": phase,
                "prior_owner": prior_owner,
                "requested_owner": requested_owner,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        phase = MacosDaemonHandoverPhase(d.pop("phase"))

        prior_owner = MacosCapabilityOwner(d.pop("prior_owner"))

        requested_owner = MacosCapabilityOwner(d.pop("requested_owner"))

        macos_daemon_owner_recovery_required_status = cls(
            phase=phase,
            prior_owner=prior_owner,
            requested_owner=requested_owner,
        )

        macos_daemon_owner_recovery_required_status.additional_properties = d
        return macos_daemon_owner_recovery_required_status

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
