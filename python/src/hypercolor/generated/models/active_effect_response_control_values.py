from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.control_value_type_0 import ControlValueType0
    from ..models.control_value_type_1 import ControlValueType1
    from ..models.control_value_type_2 import ControlValueType2
    from ..models.control_value_type_3 import ControlValueType3
    from ..models.control_value_type_4 import ControlValueType4
    from ..models.control_value_type_5 import ControlValueType5
    from ..models.control_value_type_6 import ControlValueType6
    from ..models.control_value_type_7 import ControlValueType7


T = TypeVar("T", bound="ActiveEffectResponseControlValues")


@_attrs_define
class ActiveEffectResponseControlValues:
    """ """

    additional_properties: dict[
        str,
        ControlValueType0
        | ControlValueType1
        | ControlValueType2
        | ControlValueType3
        | ControlValueType4
        | ControlValueType5
        | ControlValueType6
        | ControlValueType7,
    ] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.control_value_type_0 import ControlValueType0
        from ..models.control_value_type_1 import ControlValueType1
        from ..models.control_value_type_2 import ControlValueType2
        from ..models.control_value_type_3 import ControlValueType3
        from ..models.control_value_type_4 import ControlValueType4
        from ..models.control_value_type_5 import ControlValueType5
        from ..models.control_value_type_6 import ControlValueType6

        field_dict: dict[str, Any] = {}
        for prop_name, prop in self.additional_properties.items():
            if isinstance(prop, ControlValueType0):
                field_dict[prop_name] = prop.to_dict()
            elif isinstance(prop, ControlValueType1):
                field_dict[prop_name] = prop.to_dict()
            elif isinstance(prop, ControlValueType2):
                field_dict[prop_name] = prop.to_dict()
            elif isinstance(prop, ControlValueType3):
                field_dict[prop_name] = prop.to_dict()
            elif isinstance(prop, ControlValueType4):
                field_dict[prop_name] = prop.to_dict()
            elif isinstance(prop, ControlValueType5):
                field_dict[prop_name] = prop.to_dict()
            elif isinstance(prop, ControlValueType6):
                field_dict[prop_name] = prop.to_dict()
            else:
                field_dict[prop_name] = prop.to_dict()

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
        active_effect_response_control_values = cls()

        additional_properties = {}
        for prop_name, prop_dict in d.items():

            def _parse_additional_property(
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

            additional_property = _parse_additional_property(prop_dict)

            additional_properties[prop_name] = additional_property

        active_effect_response_control_values.additional_properties = (
            additional_properties
        )
        return active_effect_response_control_values

    @property
    def additional_keys(self) -> list[str]:
        return list(self.additional_properties.keys())

    def __getitem__(
        self, key: str
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
        return self.additional_properties[key]

    def __setitem__(
        self,
        key: str,
        value: ControlValueType0
        | ControlValueType1
        | ControlValueType2
        | ControlValueType3
        | ControlValueType4
        | ControlValueType5
        | ControlValueType6
        | ControlValueType7,
    ) -> None:
        self.additional_properties[key] = value

    def __delitem__(self, key: str) -> None:
        del self.additional_properties[key]

    def __contains__(self, key: str) -> bool:
        return key in self.additional_properties
