from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.apply_policy_type_2_kind import ApplyPolicyType2Kind

T = TypeVar("T", bound="ApplyPolicyType2")


@_attrs_define
class ApplyPolicyType2:
    """The discovery worker resolves it on its next tick.

    Attributes:
        kind (ApplyPolicyType2Kind):
    """

    kind: ApplyPolicyType2Kind
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        kind = self.kind.value

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "kind": kind,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        kind = ApplyPolicyType2Kind(d.pop("kind"))

        apply_policy_type_2 = cls(
            kind=kind,
        )

        apply_policy_type_2.additional_properties = d
        return apply_policy_type_2

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
