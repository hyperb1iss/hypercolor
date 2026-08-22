from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="DiscoveryScannerResult")


@_attrs_define
class DiscoveryScannerResult:
    """Per-scanner diagnostics from a completed discovery scan.

    Attributes:
        discovered (int):
        duration_ms (int):
        scanner (str):
        status (str):
        error (None | str | Unset):
    """

    discovered: int
    duration_ms: int
    scanner: str
    status: str
    error: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        discovered = self.discovered

        duration_ms = self.duration_ms

        scanner = self.scanner

        status = self.status

        error: None | str | Unset
        if isinstance(self.error, Unset):
            error = UNSET
        else:
            error = self.error

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "discovered": discovered,
                "duration_ms": duration_ms,
                "scanner": scanner,
                "status": status,
            }
        )
        if error is not UNSET:
            field_dict["error"] = error

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        discovered = d.pop("discovered")

        duration_ms = d.pop("duration_ms")

        scanner = d.pop("scanner")

        status = d.pop("status")

        def _parse_error(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        error = _parse_error(d.pop("error", UNSET))

        discovery_scanner_result = cls(
            discovered=discovered,
            duration_ms=duration_ms,
            scanner=scanner,
            status=status,
            error=error,
        )

        discovery_scanner_result.additional_properties = d
        return discovery_scanner_result

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
