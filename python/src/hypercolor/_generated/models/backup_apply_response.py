from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast
from uuid import UUID

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

T = TypeVar("T", bound="BackupApplyResponse")


@_attrs_define
class BackupApplyResponse:
    """
    Attributes:
        operation_id (UUID):
        unresolved_devices (list[str]):
    """

    operation_id: UUID
    unresolved_devices: list[str]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        operation_id = str(self.operation_id)

        unresolved_devices = self.unresolved_devices

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "operation_id": operation_id,
                "unresolved_devices": unresolved_devices,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        operation_id = UUID(d.pop("operation_id"))

        unresolved_devices = cast(list[str], d.pop("unresolved_devices"))

        backup_apply_response = cls(
            operation_id=operation_id,
            unresolved_devices=unresolved_devices,
        )

        backup_apply_response.additional_properties = d
        return backup_apply_response

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
