from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.error_body import ErrorBody
    from ..models.meta import Meta


T = TypeVar("T", bound="ApiErrorResponse")


@_attrs_define
class ApiErrorResponse:
    """Standard error response wrapper.

    Attributes:
        error (ErrorBody): Error detail payload.
        meta (Meta): Response metadata included in every envelope.
    """

    error: ErrorBody
    meta: Meta
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        error = self.error.to_dict()

        meta = self.meta.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "error": error,
                "meta": meta,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.error_body import ErrorBody
        from ..models.meta import Meta

        d = dict(src_dict)
        error = ErrorBody.from_dict(d.pop("error"))

        meta = Meta.from_dict(d.pop("meta"))

        api_error_response = cls(
            error=error,
            meta=meta,
        )

        api_error_response.additional_properties = d
        return api_error_response

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
