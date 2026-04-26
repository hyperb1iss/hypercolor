from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

T = TypeVar("T", bound="Meta")


@_attrs_define
class Meta:
    """Response metadata included in every envelope.

    Attributes:
        api_version (str): API version string.
        request_id (str): Per-request correlation ID, prefixed `req_`.
        timestamp (str): ISO 8601 UTC timestamp of response generation.
    """

    api_version: str
    request_id: str
    timestamp: str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        api_version = self.api_version

        request_id = self.request_id

        timestamp = self.timestamp

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "api_version": api_version,
                "request_id": request_id,
                "timestamp": timestamp,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        api_version = d.pop("api_version")

        request_id = d.pop("request_id")

        timestamp = d.pop("timestamp")

        meta = cls(
            api_version=api_version,
            request_id=request_id,
            timestamp=timestamp,
        )

        meta.additional_properties = d
        return meta

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
