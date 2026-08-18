from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.apply_policy_type_1_kind import ApplyPolicyType1Kind

T = TypeVar("T", bound="ApplyPolicyType1")


@_attrs_define
class ApplyPolicyType1:
    """Read fresh on every use — takes effect on the next read with no
    dispatch (session policy, per-scene media admission, driver
    settings).

        Attributes:
            kind (ApplyPolicyType1Kind):
    """

    kind: ApplyPolicyType1Kind
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
        kind = ApplyPolicyType1Kind(d.pop("kind"))

        apply_policy_type_1 = cls(
            kind=kind,
        )

        apply_policy_type_1.additional_properties = d
        return apply_policy_type_1

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
