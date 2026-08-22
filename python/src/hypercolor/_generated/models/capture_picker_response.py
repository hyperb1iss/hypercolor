from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

T = TypeVar("T", bound="CapturePickerResponse")


@_attrs_define
class CapturePickerResponse:
    """
    Attributes:
        grant_owner (str):
        picking (bool):
    """

    grant_owner: str
    picking: bool
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        grant_owner = self.grant_owner

        picking = self.picking

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "grant_owner": grant_owner,
                "picking": picking,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        grant_owner = d.pop("grant_owner")

        picking = d.pop("picking")

        capture_picker_response = cls(
            grant_owner=grant_owner,
            picking=picking,
        )

        capture_picker_response.additional_properties = d
        return capture_picker_response

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
