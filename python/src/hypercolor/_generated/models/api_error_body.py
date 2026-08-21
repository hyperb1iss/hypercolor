from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define

if TYPE_CHECKING:
    from ..models.api_error_detail import ApiErrorDetail
    from ..models.response_meta import ResponseMeta


T = TypeVar("T", bound="ApiErrorBody")


@_attrs_define
class ApiErrorBody:
    """Standard error envelope: `{ error: { code, message, details }, meta }`.

    Attributes:
        error (ApiErrorDetail): The error payload inside [`ApiErrorBody`].
        meta (ResponseMeta): Response metadata included in every envelope.
    """

    error: ApiErrorDetail
    meta: ResponseMeta

    def to_dict(self) -> dict[str, Any]:
        error = self.error.to_dict()

        meta = self.meta.to_dict()

        field_dict: dict[str, Any] = {}

        field_dict.update(
            {
                "error": error,
                "meta": meta,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.api_error_detail import ApiErrorDetail
        from ..models.response_meta import ResponseMeta

        d = dict(src_dict)
        error = ApiErrorDetail.from_dict(d.pop("error"))

        meta = ResponseMeta.from_dict(d.pop("meta"))

        api_error_body = cls(
            error=error,
            meta=meta,
        )

        return api_error_body
