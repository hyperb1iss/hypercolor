from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.asset_warning_type_0_kind import AssetWarningType0Kind

T = TypeVar("T", bound="AssetWarningType0")


@_attrs_define
class AssetWarningType0:
    """
    Attributes:
        kind (AssetWarningType0Kind):
        limit_bytes (int):
    """

    kind: AssetWarningType0Kind
    limit_bytes: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        kind = self.kind.value

        limit_bytes = self.limit_bytes

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "kind": kind,
                "limit_bytes": limit_bytes,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        kind = AssetWarningType0Kind(d.pop("kind"))

        limit_bytes = d.pop("limit_bytes")

        asset_warning_type_0 = cls(
            kind=kind,
            limit_bytes=limit_bytes,
        )

        asset_warning_type_0.additional_properties = d
        return asset_warning_type_0

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
