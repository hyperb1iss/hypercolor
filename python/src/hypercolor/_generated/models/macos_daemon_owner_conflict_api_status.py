from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.macos_capability_owner_api import MacosCapabilityOwnerApi

T = TypeVar("T", bound="MacosDaemonOwnerConflictApiStatus")


@_attrs_define
class MacosDaemonOwnerConflictApiStatus:
    """
    Attributes:
        active (MacosCapabilityOwnerApi):
        contender (MacosCapabilityOwnerApi):
        observed_at_ms (int):
    """

    active: MacosCapabilityOwnerApi
    contender: MacosCapabilityOwnerApi
    observed_at_ms: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        active = self.active.value

        contender = self.contender.value

        observed_at_ms = self.observed_at_ms

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "active": active,
                "contender": contender,
                "observed_at_ms": observed_at_ms,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        active = MacosCapabilityOwnerApi(d.pop("active"))

        contender = MacosCapabilityOwnerApi(d.pop("contender"))

        observed_at_ms = d.pop("observed_at_ms")

        macos_daemon_owner_conflict_api_status = cls(
            active=active,
            contender=contender,
            observed_at_ms=observed_at_ms,
        )

        macos_daemon_owner_conflict_api_status.additional_properties = d
        return macos_daemon_owner_conflict_api_status

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
