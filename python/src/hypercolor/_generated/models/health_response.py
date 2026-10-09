from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.daemon_startup_progress import DaemonStartupProgress
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
        startup (DaemonStartupProgress | None | Unset):
    """

    checks: HealthChecks
    status: str
    uptime_seconds: int
    version: str
    startup: DaemonStartupProgress | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.daemon_startup_progress import (
            DaemonStartupProgress,
        )

        checks = self.checks.to_dict()

        status = self.status

        uptime_seconds = self.uptime_seconds

        version = self.version

        startup: dict[str, Any] | None | Unset
        if isinstance(self.startup, Unset):
            startup = UNSET
        elif isinstance(self.startup, DaemonStartupProgress):
            startup = self.startup.to_dict()
        else:
            startup = self.startup

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
        if startup is not UNSET:
            field_dict["startup"] = startup

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.daemon_startup_progress import (
            DaemonStartupProgress,
        )
        from ..models.health_checks import HealthChecks

        d = dict(src_dict)
        checks = HealthChecks.from_dict(d.pop("checks"))

        status = d.pop("status")

        uptime_seconds = d.pop("uptime_seconds")

        version = d.pop("version")

        def _parse_startup(data: object) -> DaemonStartupProgress | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                startup_type_0 = DaemonStartupProgress.from_dict(data)

                return startup_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(DaemonStartupProgress | None | Unset, data)

        startup = _parse_startup(d.pop("startup", UNSET))

        health_response = cls(
            checks=checks,
            status=status,
            uptime_seconds=uptime_seconds,
            version=version,
            startup=startup,
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
