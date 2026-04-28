from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.applied_control_change_value import AppliedControlChangeValue


T = TypeVar("T", bound="AppliedControlChange")


@_attrs_define
class AppliedControlChange:
    """Accepted field change.

    Attributes:
        field_id (str):
        value (AppliedControlChangeValue): Applied value.
    """

    field_id: str
    value: AppliedControlChangeValue
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        field_id = self.field_id

        value = self.value.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "field_id": field_id,
                "value": value,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.applied_control_change_value import AppliedControlChangeValue

        d = dict(src_dict)
        field_id = d.pop("field_id")

        value = AppliedControlChangeValue.from_dict(d.pop("value"))

        applied_control_change = cls(
            field_id=field_id,
            value=value,
        )

        applied_control_change.additional_properties = d
        return applied_control_change

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
