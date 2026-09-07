from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define

from ..types import UNSET, Unset

T = TypeVar("T", bound="ApiErrorDetail")


@_attrs_define
class ApiErrorDetail:
    """The error payload inside [`ApiErrorBody`].

    Attributes:
        code (str): Stable machine-readable error code (snake_case).
        message (str): Human-readable message.
        details (Any | Unset): Optional structured detail (validation fields, current
            versions on precondition failures).
    """

    code: str
    message: str
    details: Any | Unset = UNSET

    def to_dict(self) -> dict[str, Any]:
        code = self.code

        message = self.message

        details = self.details

        field_dict: dict[str, Any] = {}

        field_dict.update(
            {
                "code": code,
                "message": message,
            }
        )
        if details is not UNSET:
            field_dict["details"] = details

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        code = d.pop("code")

        message = d.pop("message")

        details = d.pop("details", UNSET)

        api_error_detail = cls(
            code=code,
            message=message,
            details=details,
        )

        return api_error_detail
