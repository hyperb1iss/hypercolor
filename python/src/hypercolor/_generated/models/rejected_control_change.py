from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.control_apply_error import ControlApplyError
    from ..models.rejected_control_change_attempted_value import (
        RejectedControlChangeAttemptedValue,
    )


T = TypeVar("T", bound="RejectedControlChange")


@_attrs_define
class RejectedControlChange:
    """Rejected field change.

    Attributes:
        attempted_value (RejectedControlChangeAttemptedValue): Attempted value.
        error (ControlApplyError): Typed control apply error.
        field_id (str):
    """

    attempted_value: RejectedControlChangeAttemptedValue
    error: ControlApplyError
    field_id: str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        attempted_value = self.attempted_value.to_dict()

        error = self.error.to_dict()

        field_id = self.field_id

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "attempted_value": attempted_value,
                "error": error,
                "field_id": field_id,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.control_apply_error import ControlApplyError
        from ..models.rejected_control_change_attempted_value import (
            RejectedControlChangeAttemptedValue,
        )

        d = dict(src_dict)
        attempted_value = RejectedControlChangeAttemptedValue.from_dict(
            d.pop("attempted_value")
        )

        error = ControlApplyError.from_dict(d.pop("error"))

        field_id = d.pop("field_id")

        rejected_control_change = cls(
            attempted_value=attempted_value,
            error=error,
            field_id=field_id,
        )

        rejected_control_change.additional_properties = d
        return rejected_control_change

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
