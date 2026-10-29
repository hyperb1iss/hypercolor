from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar
from uuid import UUID

from attrs import define as _attrs_define
from typing_extensions import Self

T = TypeVar("T", bound="BackupApplyRequest")


@_attrs_define
class BackupApplyRequest:
    """
    Attributes:
        operation_id (UUID):
    """

    operation_id: UUID

    def to_dict(self) -> dict[str, Any]:
        operation_id = str(self.operation_id)

        field_dict: dict[str, Any] = {}

        field_dict.update(
            {
                "operation_id": operation_id,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        operation_id = UUID(d.pop("operation_id"))

        backup_apply_request = cls(
            operation_id=operation_id,
        )

        return backup_apply_request
