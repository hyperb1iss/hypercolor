from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.control_object_field_default_value_type_0 import (
        ControlObjectFieldDefaultValueType0,
    )
    from ..models.control_object_field_value_type import ControlObjectFieldValueType


T = TypeVar("T", bound="ControlObjectField")


@_attrs_define
class ControlObjectField:
    """Field inside an object control value.

    Attributes:
        id (str): Stable field identifier.
        label (str): Human-readable label.
        required (bool): Whether this field is required.
        value_type (ControlObjectFieldValueType): Expected value type.
        default_value (ControlObjectFieldDefaultValueType0 | None | Unset): Optional default value.
    """

    id: str
    label: str
    required: bool
    value_type: ControlObjectFieldValueType
    default_value: ControlObjectFieldDefaultValueType0 | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.control_object_field_default_value_type_0 import (
            ControlObjectFieldDefaultValueType0,
        )

        id = self.id

        label = self.label

        required = self.required

        value_type = self.value_type.to_dict()

        default_value: dict[str, Any] | None | Unset
        if isinstance(self.default_value, Unset):
            default_value = UNSET
        elif isinstance(self.default_value, ControlObjectFieldDefaultValueType0):
            default_value = self.default_value.to_dict()
        else:
            default_value = self.default_value

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "id": id,
                "label": label,
                "required": required,
                "value_type": value_type,
            }
        )
        if default_value is not UNSET:
            field_dict["default_value"] = default_value

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.control_object_field_default_value_type_0 import (
            ControlObjectFieldDefaultValueType0,
        )
        from ..models.control_object_field_value_type import ControlObjectFieldValueType

        d = dict(src_dict)
        id = d.pop("id")

        label = d.pop("label")

        required = d.pop("required")

        value_type = ControlObjectFieldValueType.from_dict(d.pop("value_type"))

        def _parse_default_value(
            data: object,
        ) -> ControlObjectFieldDefaultValueType0 | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                default_value_type_0 = ControlObjectFieldDefaultValueType0.from_dict(
                    data
                )

                return default_value_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(ControlObjectFieldDefaultValueType0 | None | Unset, data)

        default_value = _parse_default_value(d.pop("default_value", UNSET))

        control_object_field = cls(
            id=id,
            label=label,
            required=required,
            value_type=value_type,
            default_value=default_value,
        )

        control_object_field.additional_properties = d
        return control_object_field

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
