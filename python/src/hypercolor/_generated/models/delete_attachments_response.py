from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

T = TypeVar("T", bound="DeleteAttachmentsResponse")


@_attrs_define
class DeleteAttachmentsResponse:
    """Response for `DELETE /api/v1/devices/{id}/attachments`.

    `deleted` is false when the device had no stored profile to remove,
    which is a success rather than a 404.

        Attributes:
            deleted (bool):
            device_id (str):
    """

    deleted: bool
    device_id: str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        deleted = self.deleted

        device_id = self.device_id

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "deleted": deleted,
                "device_id": device_id,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        deleted = d.pop("deleted")

        device_id = d.pop("device_id")

        delete_attachments_response = cls(
            deleted=deleted,
            device_id=device_id,
        )

        delete_attachments_response.additional_properties = d
        return delete_attachments_response

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
