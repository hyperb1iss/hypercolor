from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.effect_control_value_type_0 import EffectControlValueType0
    from ..models.effect_control_value_type_1 import EffectControlValueType1
    from ..models.effect_control_value_type_2 import EffectControlValueType2
    from ..models.effect_control_value_type_3 import EffectControlValueType3
    from ..models.effect_control_value_type_4 import EffectControlValueType4
    from ..models.effect_control_value_type_5 import EffectControlValueType5
    from ..models.effect_control_value_type_6 import EffectControlValueType6
    from ..models.effect_control_value_type_7 import EffectControlValueType7


T = TypeVar("T", bound="SetDisplayFaceRequestControls")


@_attrs_define
class SetDisplayFaceRequestControls:
    """ """

    additional_properties: dict[
        str,
        EffectControlValueType0
        | EffectControlValueType1
        | EffectControlValueType2
        | EffectControlValueType3
        | EffectControlValueType4
        | EffectControlValueType5
        | EffectControlValueType6
        | EffectControlValueType7,
    ] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.effect_control_value_type_0 import EffectControlValueType0
        from ..models.effect_control_value_type_1 import EffectControlValueType1
        from ..models.effect_control_value_type_2 import EffectControlValueType2
        from ..models.effect_control_value_type_3 import EffectControlValueType3
        from ..models.effect_control_value_type_4 import EffectControlValueType4
        from ..models.effect_control_value_type_5 import EffectControlValueType5
        from ..models.effect_control_value_type_6 import EffectControlValueType6

        field_dict: dict[str, Any] = {}
        for prop_name, prop in self.additional_properties.items():
            if isinstance(prop, EffectControlValueType0):
                field_dict[prop_name] = prop.to_dict()
            elif isinstance(prop, EffectControlValueType1):
                field_dict[prop_name] = prop.to_dict()
            elif isinstance(prop, EffectControlValueType2):
                field_dict[prop_name] = prop.to_dict()
            elif isinstance(prop, EffectControlValueType3):
                field_dict[prop_name] = prop.to_dict()
            elif isinstance(prop, EffectControlValueType4):
                field_dict[prop_name] = prop.to_dict()
            elif isinstance(prop, EffectControlValueType5):
                field_dict[prop_name] = prop.to_dict()
            elif isinstance(prop, EffectControlValueType6):
                field_dict[prop_name] = prop.to_dict()
            else:
                field_dict[prop_name] = prop.to_dict()

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.effect_control_value_type_0 import EffectControlValueType0
        from ..models.effect_control_value_type_1 import EffectControlValueType1
        from ..models.effect_control_value_type_2 import EffectControlValueType2
        from ..models.effect_control_value_type_3 import EffectControlValueType3
        from ..models.effect_control_value_type_4 import EffectControlValueType4
        from ..models.effect_control_value_type_5 import EffectControlValueType5
        from ..models.effect_control_value_type_6 import EffectControlValueType6
        from ..models.effect_control_value_type_7 import EffectControlValueType7

        d = dict(src_dict)
        set_display_face_request_controls = cls()

        additional_properties = {}
        for prop_name, prop_dict in d.items():

            def _parse_additional_property(
                data: object,
            ) -> (
                EffectControlValueType0
                | EffectControlValueType1
                | EffectControlValueType2
                | EffectControlValueType3
                | EffectControlValueType4
                | EffectControlValueType5
                | EffectControlValueType6
                | EffectControlValueType7
            ):
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_effect_control_value_type_0 = (
                        EffectControlValueType0.from_dict(data)
                    )

                    return componentsschemas_effect_control_value_type_0
                except (TypeError, ValueError, AttributeError, KeyError):
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_effect_control_value_type_1 = (
                        EffectControlValueType1.from_dict(data)
                    )

                    return componentsschemas_effect_control_value_type_1
                except (TypeError, ValueError, AttributeError, KeyError):
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_effect_control_value_type_2 = (
                        EffectControlValueType2.from_dict(data)
                    )

                    return componentsschemas_effect_control_value_type_2
                except (TypeError, ValueError, AttributeError, KeyError):
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_effect_control_value_type_3 = (
                        EffectControlValueType3.from_dict(data)
                    )

                    return componentsschemas_effect_control_value_type_3
                except (TypeError, ValueError, AttributeError, KeyError):
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_effect_control_value_type_4 = (
                        EffectControlValueType4.from_dict(data)
                    )

                    return componentsschemas_effect_control_value_type_4
                except (TypeError, ValueError, AttributeError, KeyError):
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_effect_control_value_type_5 = (
                        EffectControlValueType5.from_dict(data)
                    )

                    return componentsschemas_effect_control_value_type_5
                except (TypeError, ValueError, AttributeError, KeyError):
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_effect_control_value_type_6 = (
                        EffectControlValueType6.from_dict(data)
                    )

                    return componentsschemas_effect_control_value_type_6
                except (TypeError, ValueError, AttributeError, KeyError):
                    pass
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_effect_control_value_type_7 = (
                    EffectControlValueType7.from_dict(data)
                )

                return componentsschemas_effect_control_value_type_7

            additional_property = _parse_additional_property(prop_dict)

            additional_properties[prop_name] = additional_property

        set_display_face_request_controls.additional_properties = additional_properties
        return set_display_face_request_controls

    @property
    def additional_keys(self) -> list[str]:
        return list(self.additional_properties.keys())

    def __getitem__(
        self, key: str
    ) -> (
        EffectControlValueType0
        | EffectControlValueType1
        | EffectControlValueType2
        | EffectControlValueType3
        | EffectControlValueType4
        | EffectControlValueType5
        | EffectControlValueType6
        | EffectControlValueType7
    ):
        return self.additional_properties[key]

    def __setitem__(
        self,
        key: str,
        value: EffectControlValueType0
        | EffectControlValueType1
        | EffectControlValueType2
        | EffectControlValueType3
        | EffectControlValueType4
        | EffectControlValueType5
        | EffectControlValueType6
        | EffectControlValueType7,
    ) -> None:
        self.additional_properties[key] = value

    def __delitem__(self, key: str) -> None:
        del self.additional_properties[key]

    def __contains__(self, key: str) -> bool:
        return key in self.additional_properties
