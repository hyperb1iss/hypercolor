from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="InputSourceIssueStatus")


@_attrs_define
class InputSourceIssueStatus:
    """Structured source issue safe for operational status surfaces.

    Attributes:
        code (str):
        message (str):
        retryable (bool):
        remediation (None | str | Unset):
    """

    code: str
    message: str
    retryable: bool
    remediation: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        code = self.code

        message = self.message

        retryable = self.retryable

        remediation: None | str | Unset
        if isinstance(self.remediation, Unset):
            remediation = UNSET
        else:
            remediation = self.remediation

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "code": code,
                "message": message,
                "retryable": retryable,
            }
        )
        if remediation is not UNSET:
            field_dict["remediation"] = remediation

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        code = d.pop("code")

        message = d.pop("message")

        retryable = d.pop("retryable")

        def _parse_remediation(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        remediation = _parse_remediation(d.pop("remediation", UNSET))

        input_source_issue_status = cls(
            code=code,
            message=message,
            retryable=retryable,
            remediation=remediation,
        )

        input_source_issue_status.additional_properties = d
        return input_source_issue_status

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
