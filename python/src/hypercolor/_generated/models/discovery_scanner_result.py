from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..types import UNSET, Unset

T = TypeVar("T", bound="DiscoveryScannerResult")


@_attrs_define
class DiscoveryScannerResult:
    """Per-scanner diagnostics from a completed discovery scan.

    Attributes:
        discovered (int):
        duration_ms (int):
        scanner (str):
        status (str): `ok`, `partial` when the scan may have missed devices, or `error`.
        error (None | str | Unset):
        gaps (list[str] | Unset): What a `partial` scan may have missed. Its devices stay tracked.
    """

    discovered: int
    duration_ms: int
    scanner: str
    status: str
    error: None | str | Unset = UNSET
    gaps: list[str] | Unset = UNSET
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

        gaps: list[str] | Unset = UNSET
        if not isinstance(self.gaps, Unset):
            gaps = self.gaps

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
        if gaps is not UNSET:
            field_dict["gaps"] = gaps

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
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

        gaps = cast(list[str], d.pop("gaps", UNSET))

        discovery_scanner_result = cls(
            discovered=discovered,
            duration_ms=duration_ms,
            scanner=scanner,
            status=status,
            error=error,
            gaps=gaps,
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
