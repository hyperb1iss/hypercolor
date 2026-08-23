from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

T = TypeVar("T", bound="RescanResponse")


@_attrs_define
class RescanResponse:
    """Response for `POST /api/v1/effects/rescan`.

    Counts describe what the rescan changed in the registry, so an
    all-zero response means the effect directories were already current.

        Attributes:
            added (int):
            removed (int):
            updated (int):
    """

    added: int
    removed: int
    updated: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        added = self.added

        removed = self.removed

        updated = self.updated

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "added": added,
                "removed": removed,
                "updated": updated,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        added = d.pop("added")

        removed = d.pop("removed")

        updated = d.pop("updated")

        rescan_response = cls(
            added=added,
            removed=removed,
            updated=updated,
        )

        rescan_response.additional_properties = d
        return rescan_response

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
