from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar
from uuid import UUID

from attrs import define as _attrs_define
from attrs import field as _attrs_field

T = TypeVar("T", bound="DeleteSimulatedDisplayResponse")


@_attrs_define
class DeleteSimulatedDisplayResponse:
    """Response from `DELETE /api/v1/simulators/displays/{id}`.

    Attributes:
        deleted (bool):
        id (UUID): Opaque, globally unique device identifier.

            Wraps a `UUIDv7` so identifiers are time-ordered and safe to use as
            database keys, map keys, and log correlation IDs.
    """

    deleted: bool
    id: UUID
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        deleted = self.deleted

        id = str(self.id)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "deleted": deleted,
                "id": id,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        deleted = d.pop("deleted")

        id = UUID(d.pop("id"))

        delete_simulated_display_response = cls(
            deleted=deleted,
            id=id,
        )

        delete_simulated_display_response.additional_properties = d
        return delete_simulated_display_response

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
