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
        unassigned_behavior (str):
        zones_revision (int):
    """

    unassigned_behavior: str
    zones_revision: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        unassigned_behavior = self.unassigned_behavior

        zones_revision = self.zones_revision

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "unassigned_behavior": unassigned_behavior,
                "zones_revision": zones_revision,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        unassigned_behavior = d.pop("unassigned_behavior")

        zones_revision = d.pop("zones_revision")

        unassigned_behavior_response = cls(
            unassigned_behavior=unassigned_behavior,
            zones_revision=zones_revision,
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
