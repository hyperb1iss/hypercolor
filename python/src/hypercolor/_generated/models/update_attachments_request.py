from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.component_binding import ComponentBinding


T = TypeVar("T", bound="UpdateAttachmentsRequest")


@_attrs_define
class UpdateAttachmentsRequest:
    """Request body for `PUT /api/v1/devices/{id}/attachments`.

    The binding list replaces the device's attachments wholesale.

        Attributes:
            bindings (list[ComponentBinding] | Unset):
            validate_only (bool | Unset): Validate and resolve the profile without applying any side effects.
    """

    bindings: list[ComponentBinding] | Unset = UNSET
    validate_only: bool | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        bindings: list[dict[str, Any]] | Unset = UNSET
        if not isinstance(self.bindings, Unset):
            bindings = []
            for bindings_item_data in self.bindings:
                bindings_item = bindings_item_data.to_dict()
                bindings.append(bindings_item)

        validate_only = self.validate_only

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if bindings is not UNSET:
            field_dict["bindings"] = bindings
        if validate_only is not UNSET:
            field_dict["validate_only"] = validate_only

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.component_binding import ComponentBinding

        d = dict(src_dict)
        _bindings = d.pop("bindings", UNSET)
        bindings: list[ComponentBinding] | Unset = UNSET
        if _bindings is not UNSET:
            bindings = []
            for bindings_item_data in _bindings:
                bindings_item = ComponentBinding.from_dict(bindings_item_data)

                bindings.append(bindings_item)

        validate_only = d.pop("validate_only", UNSET)

        update_attachments_request = cls(
            bindings=bindings,
            validate_only=validate_only,
        )

        update_attachments_request.additional_properties = d
        return update_attachments_request

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
