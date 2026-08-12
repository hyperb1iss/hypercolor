from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.macos_selection_state_api_type_2_type import (
    MacosSelectionStateApiType2Type,
)

T = TypeVar("T", bound="MacosSelectionStateApiType2")


@_attrs_define
class MacosSelectionStateApiType2:
    """
    Attributes:
        content_style (str):
        type_ (MacosSelectionStateApiType2Type):
    """

    content_style: str
    type_: MacosSelectionStateApiType2Type
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        content_style = self.content_style

        type_ = self.type_.value

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "content_style": content_style,
                "type": type_,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        content_style = d.pop("content_style")

        type_ = MacosSelectionStateApiType2Type(d.pop("type"))

        macos_selection_state_api_type_2 = cls(
            content_style=content_style,
            type_=type_,
        )

        macos_selection_state_api_type_2.additional_properties = d
        return macos_selection_state_api_type_2

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
