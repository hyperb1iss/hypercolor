from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.protected_source_grant_owner import ProtectedSourceGrantOwner

T = TypeVar("T", bound="ApiResponseCapturePickerResponseData")


@_attrs_define
class ApiResponseCapturePickerResponseData:
    """
    Attributes:
        grant_owner (ProtectedSourceGrantOwner):
        picking (bool):
    """

    grant_owner: ProtectedSourceGrantOwner
    picking: bool
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        grant_owner = self.grant_owner.value

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
        grant_owner = ProtectedSourceGrantOwner(d.pop("grant_owner"))

        picking = d.pop("picking")

        api_response_capture_picker_response_data = cls(
            grant_owner=grant_owner,
            picking=picking,
        )

        api_response_capture_picker_response_data.additional_properties = d
        return api_response_capture_picker_response_data

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
