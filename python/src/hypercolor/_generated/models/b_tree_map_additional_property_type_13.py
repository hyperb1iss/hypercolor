from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.b_tree_map_additional_property_type_13_kind import (
    BTreeMapAdditionalPropertyType13Kind,
)

if TYPE_CHECKING:
    from ..models.control_value_type_0 import ControlValueType0
    from ..models.control_value_type_1 import ControlValueType1
    from ..models.control_value_type_2 import ControlValueType2
    from ..models.control_value_type_3 import ControlValueType3
    from ..models.control_value_type_4 import ControlValueType4
    from ..models.control_value_type_5 import ControlValueType5
    from ..models.control_value_type_6 import ControlValueType6
    from ..models.control_value_type_7 import ControlValueType7


T = TypeVar("T", bound="BTreeMapAdditionalPropertyType13")


@_attrs_define
class BTreeMapAdditionalPropertyType13:
    """Homogeneous list.

    Attributes:
        kind (BTreeMapAdditionalPropertyType13Kind):
        value (list[ControlValueType0 | ControlValueType1 | ControlValueType2 | ControlValueType3 | ControlValueType4 |
            ControlValueType5 | ControlValueType6 | ControlValueType7]): Homogeneous list.
    """

    kind: BTreeMapAdditionalPropertyType13Kind
    value: list[
        ControlValueType0
        | ControlValueType1
        | ControlValueType2
        | ControlValueType3
        | ControlValueType4
        | ControlValueType5
        | ControlValueType6
        | ControlValueType7
    ]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.control_value_type_0 import ControlValueType0
        from ..models.control_value_type_1 import ControlValueType1
        from ..models.control_value_type_2 import ControlValueType2
        from ..models.control_value_type_3 import ControlValueType3
        from ..models.control_value_type_4 import ControlValueType4
        from ..models.control_value_type_5 import ControlValueType5
        from ..models.control_value_type_6 import ControlValueType6

        kind = self.kind.value

        value = []
        for value_item_data in self.value:
            value_item: dict[str, Any]
            if isinstance(value_item_data, ControlValueType0):
                value_item = value_item_data.to_dict()
            elif isinstance(value_item_data, ControlValueType1):
                value_item = value_item_data.to_dict()
            elif isinstance(value_item_data, ControlValueType2):
                value_item = value_item_data.to_dict()
            elif isinstance(value_item_data, ControlValueType3):
                value_item = value_item_data.to_dict()
            elif isinstance(value_item_data, ControlValueType4):
                value_item = value_item_data.to_dict()
            elif isinstance(value_item_data, ControlValueType5):
                value_item = value_item_data.to_dict()
            elif isinstance(value_item_data, ControlValueType6):
                value_item = value_item_data.to_dict()
            else:
                value_item = value_item_data.to_dict()

            value.append(value_item)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "kind": kind,
                "value": value,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.control_value_type_0 import ControlValueType0
        from ..models.control_value_type_1 import ControlValueType1
        from ..models.control_value_type_2 import ControlValueType2
        from ..models.control_value_type_3 import ControlValueType3
        from ..models.control_value_type_4 import ControlValueType4
        from ..models.control_value_type_5 import ControlValueType5
        from ..models.control_value_type_6 import ControlValueType6
        from ..models.control_value_type_7 import ControlValueType7

        d = dict(src_dict)
        kind = BTreeMapAdditionalPropertyType13Kind(d.pop("kind"))

        value = []
        _value = d.pop("value")
        for value_item_data in _value:

            def _parse_value_item(
                data: object,
            ) -> (
                ControlValueType0
                | ControlValueType1
                | ControlValueType2
                | ControlValueType3
                | ControlValueType4
                | ControlValueType5
                | ControlValueType6
                | ControlValueType7
            ):
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_control_value_type_0 = (
                        ControlValueType0.from_dict(data)
                    )

                    return componentsschemas_control_value_type_0
                except (TypeError, ValueError, AttributeError, KeyError):
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_control_value_type_1 = (
                        ControlValueType1.from_dict(data)
                    )

                    return componentsschemas_control_value_type_1
                except (TypeError, ValueError, AttributeError, KeyError):
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_control_value_type_2 = (
                        ControlValueType2.from_dict(data)
                    )

                    return componentsschemas_control_value_type_2
                except (TypeError, ValueError, AttributeError, KeyError):
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_control_value_type_3 = (
                        ControlValueType3.from_dict(data)
                    )

                    return componentsschemas_control_value_type_3
                except (TypeError, ValueError, AttributeError, KeyError):
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_control_value_type_4 = (
                        ControlValueType4.from_dict(data)
                    )

                    return componentsschemas_control_value_type_4
                except (TypeError, ValueError, AttributeError, KeyError):
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_control_value_type_5 = (
                        ControlValueType5.from_dict(data)
                    )

                    return componentsschemas_control_value_type_5
                except (TypeError, ValueError, AttributeError, KeyError):
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_control_value_type_6 = (
                        ControlValueType6.from_dict(data)
                    )

                    return componentsschemas_control_value_type_6
                except (TypeError, ValueError, AttributeError, KeyError):
                    pass
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_control_value_type_7 = ControlValueType7.from_dict(
                    data
                )

                return componentsschemas_control_value_type_7

            value_item = _parse_value_item(value_item_data)

            value.append(value_item)

        b_tree_map_additional_property_type_13 = cls(
            kind=kind,
            value=value,
        )

        b_tree_map_additional_property_type_13.additional_properties = d
        return b_tree_map_additional_property_type_13

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
