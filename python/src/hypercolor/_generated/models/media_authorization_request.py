from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.media_authorization_adapter import MediaAuthorizationAdapter

T = TypeVar("T", bound="MediaAuthorizationRequest")


@_attrs_define
class MediaAuthorizationRequest:
    """Explicit media Automation authorization request.

    Attributes:
        adapter (MediaAuthorizationAdapter):
    """

    adapter: MediaAuthorizationAdapter
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        adapter = self.adapter.value

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "adapter": adapter,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        adapter = MediaAuthorizationAdapter(d.pop("adapter"))

        media_authorization_request = cls(
            adapter=adapter,
        )

        media_authorization_request.additional_properties = d
        return media_authorization_request

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
