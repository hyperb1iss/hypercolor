from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.protected_source_grant_owner import ProtectedSourceGrantOwner

T = TypeVar("T", bound="CaptureAuthorizationResponse")


@_attrs_define
class CaptureAuthorizationResponse:
    """
    Attributes:
        authorized (bool):
        grant_owner (ProtectedSourceGrantOwner):
    """

    authorized: bool
    grant_owner: ProtectedSourceGrantOwner
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        authorized = self.authorized

        grant_owner = self.grant_owner.value

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "authorized": authorized,
                "grant_owner": grant_owner,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        authorized = d.pop("authorized")

        grant_owner = ProtectedSourceGrantOwner(d.pop("grant_owner"))

        capture_authorization_response = cls(
            authorized=authorized,
            grant_owner=grant_owner,
        )

        capture_authorization_response.additional_properties = d
        return capture_authorization_response

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
