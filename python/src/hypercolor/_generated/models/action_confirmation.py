from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.action_confirmation_level import ActionConfirmationLevel

T = TypeVar("T", bound="ActionConfirmation")


@_attrs_define
class ActionConfirmation:
    """Action confirmation metadata.

    Attributes:
        level (ActionConfirmationLevel): Confirmation severity for actions.
        message (str): Human-readable message.
    """

    level: ActionConfirmationLevel
    message: str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        level = self.level.value

        message = self.message

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "level": level,
                "message": message,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        level = ActionConfirmationLevel(d.pop("level"))

        message = d.pop("message")

        action_confirmation = cls(
            level=level,
            message=message,
        )

        action_confirmation.additional_properties = d
        return action_confirmation

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
