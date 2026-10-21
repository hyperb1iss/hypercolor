from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..types import UNSET, Unset

T = TypeVar("T", bound="GpuErrorStatus")


@_attrs_define
class GpuErrorStatus:
    """Uncaptured wgpu errors and device losses since the daemon started.

    Attributes:
        device_lost (int):
        internal_errors (int):
        out_of_memory_errors (int):
        validation_errors (int):
        last_error (None | str | Unset): `device: kind: message` of the most recent one. Absent when there
            was none, or when the caller may not read private diagnostics.
    """

    device_lost: int
    internal_errors: int
    out_of_memory_errors: int
    validation_errors: int
    last_error: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        device_lost = self.device_lost

        internal_errors = self.internal_errors

        out_of_memory_errors = self.out_of_memory_errors

        validation_errors = self.validation_errors

        last_error: None | str | Unset
        if isinstance(self.last_error, Unset):
            last_error = UNSET
        else:
            last_error = self.last_error

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "device_lost": device_lost,
                "internal_errors": internal_errors,
                "out_of_memory_errors": out_of_memory_errors,
                "validation_errors": validation_errors,
            }
        )
        if last_error is not UNSET:
            field_dict["last_error"] = last_error

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        device_lost = d.pop("device_lost")

        internal_errors = d.pop("internal_errors")

        out_of_memory_errors = d.pop("out_of_memory_errors")

        validation_errors = d.pop("validation_errors")

        def _parse_last_error(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        last_error = _parse_last_error(d.pop("last_error", UNSET))

        gpu_error_status = cls(
            device_lost=device_lost,
            internal_errors=internal_errors,
            out_of_memory_errors=out_of_memory_errors,
            validation_errors=validation_errors,
            last_error=last_error,
        )

        gpu_error_status.additional_properties = d
        return gpu_error_status

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
