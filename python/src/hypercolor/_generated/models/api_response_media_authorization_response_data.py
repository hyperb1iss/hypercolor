from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.media_authorization_adapter import MediaAuthorizationAdapter

T = TypeVar("T", bound="ApiResponseMediaAuthorizationResponseData")


@_attrs_define
class ApiResponseMediaAuthorizationResponseData:
    """Result of one explicit media Automation authorization request.

    Attributes:
        adapter (MediaAuthorizationAdapter):
        authorized (bool): Whether Automation access is authorized.
    """

    adapter: MediaAuthorizationAdapter
    authorized: bool
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        adapter = self.adapter.value

        authorized = self.authorized

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "adapter": adapter,
                "authorized": authorized,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        adapter = MediaAuthorizationAdapter(d.pop("adapter"))

        authorized = d.pop("authorized")

        api_response_media_authorization_response_data = cls(
            adapter=adapter,
            authorized=authorized,
        )

        api_response_media_authorization_response_data.additional_properties = d
        return api_response_media_authorization_response_data

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
