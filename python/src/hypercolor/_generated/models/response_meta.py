from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define

T = TypeVar("T", bound="ResponseMeta")


@_attrs_define
class ResponseMeta:
    """Response metadata included in every envelope.

    Attributes:
        api_version (str): API version string.
        request_id (str): Per-request correlation ID, prefixed `req_`.
        timestamp (str): ISO 8601 UTC timestamp of response generation.
    """

    api_version: str
    request_id: str
    timestamp: str

    def to_dict(self) -> dict[str, Any]:
        api_version = self.api_version

        request_id = self.request_id

        timestamp = self.timestamp

        field_dict: dict[str, Any] = {}

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

        response_meta = cls(
            api_version=api_version,
            request_id=request_id,
            timestamp=timestamp,
        )

        return response_meta
