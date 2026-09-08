from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.coverage_identity_kind import CoverageIdentityKind

T = TypeVar("T", bound="CoverageIdentity")


@_attrs_define
class CoverageIdentity:
    """The physical-device identity a coverage row was joined on.

    Attributes:
        kind (CoverageIdentityKind): Which key joined the sources of one coverage row.
        label (str): Best available human label for the hardware.
        value (str): The normalized key value.
    """

    kind: CoverageIdentityKind
    label: str
    value: str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        kind = self.kind.value

        label = self.label

        value = self.value

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "kind": kind,
                "label": label,
                "value": value,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        kind = CoverageIdentityKind(d.pop("kind"))

        label = d.pop("label")

        value = d.pop("value")

        coverage_identity = cls(
            kind=kind,
            label=label,
            value=value,
        )

        coverage_identity.additional_properties = d
        return coverage_identity

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
