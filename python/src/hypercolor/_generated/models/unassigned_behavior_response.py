from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

T = TypeVar("T", bound="UnassignedBehaviorResponse")


@_attrs_define
class UnassignedBehaviorResponse:
    """Response for the unassigned-behavior PATCH.

    Attributes:
        groups_revision (int):
        unassigned_behavior (str):
    """

    groups_revision: int
    unassigned_behavior: str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        groups_revision = self.groups_revision

        unassigned_behavior = self.unassigned_behavior

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "groups_revision": groups_revision,
                "unassigned_behavior": unassigned_behavior,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        groups_revision = d.pop("groups_revision")

        unassigned_behavior = d.pop("unassigned_behavior")

        unassigned_behavior_response = cls(
            groups_revision=groups_revision,
            unassigned_behavior=unassigned_behavior,
        )

        unassigned_behavior_response.additional_properties = d
        return unassigned_behavior_response

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
