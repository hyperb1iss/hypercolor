from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.health_checks import HealthChecks


T = TypeVar("T", bound="HealthResponse")


@_attrs_define
class HealthResponse:
    """
    Attributes:
        checks (HealthChecks):
        status (str):
        uptime_seconds (int):
        version (str):
    """

    checks: HealthChecks
    status: str
    uptime_seconds: int
    version: str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        checks = self.checks.to_dict()

        status = self.status

        uptime_seconds = self.uptime_seconds

        version = self.version

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "checks": checks,
                "status": status,
                "uptime_seconds": uptime_seconds,
                "version": version,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.health_checks import HealthChecks

        d = dict(src_dict)
        checks = HealthChecks.from_dict(d.pop("checks"))

        status = d.pop("status")

        uptime_seconds = d.pop("uptime_seconds")

        version = d.pop("version")

        health_response = cls(
            checks=checks,
            status=status,
            uptime_seconds=uptime_seconds,
            version=version,
        )

        health_response.additional_properties = d
        return health_response

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
