from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast
from uuid import UUID

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.missing_setup_resources import MissingSetupResources


T = TypeVar("T", bound="BackupApplyResponse")


@_attrs_define
class BackupApplyResponse:
    """
    Attributes:
        operation_id (UUID):
        unresolved_devices (list[str]):
        missing_resources (MissingSetupResources | Unset): Referenced content that must be installed or imported on this
            computer.
    """

    operation_id: UUID
    unresolved_devices: list[str]
    missing_resources: MissingSetupResources | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        operation_id = str(self.operation_id)

        unresolved_devices = self.unresolved_devices

        missing_resources: dict[str, Any] | Unset = UNSET
        if not isinstance(self.missing_resources, Unset):
            missing_resources = self.missing_resources.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "operation_id": operation_id,
                "unresolved_devices": unresolved_devices,
            }
        )
        if missing_resources is not UNSET:
            field_dict["missing_resources"] = missing_resources

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.missing_setup_resources import (
            MissingSetupResources,
        )

        d = dict(src_dict)
        operation_id = UUID(d.pop("operation_id"))

        unresolved_devices = cast(list[str], d.pop("unresolved_devices"))

        _missing_resources = d.pop("missing_resources", UNSET)
        missing_resources: MissingSetupResources | Unset
        if isinstance(_missing_resources, Unset):
            missing_resources = UNSET
        else:
            missing_resources = MissingSetupResources.from_dict(_missing_resources)

        backup_apply_response = cls(
            operation_id=operation_id,
            unresolved_devices=unresolved_devices,
            missing_resources=missing_resources,
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
