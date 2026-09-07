from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.b_tree_map_additional_property_type_0 import (
        BTreeMapAdditionalPropertyType0,
    )
    from ..models.b_tree_map_additional_property_type_1 import (
        BTreeMapAdditionalPropertyType1,
    )
    from ..models.b_tree_map_additional_property_type_2 import (
        BTreeMapAdditionalPropertyType2,
    )
    from ..models.b_tree_map_additional_property_type_3 import (
        BTreeMapAdditionalPropertyType3,
    )
    from ..models.b_tree_map_additional_property_type_4 import (
        BTreeMapAdditionalPropertyType4,
    )
    from ..models.b_tree_map_additional_property_type_5 import (
        BTreeMapAdditionalPropertyType5,
    )
    from ..models.b_tree_map_additional_property_type_6 import (
        BTreeMapAdditionalPropertyType6,
    )
    from ..models.b_tree_map_additional_property_type_7 import (
        BTreeMapAdditionalPropertyType7,
    )
    from ..models.b_tree_map_additional_property_type_8 import (
        BTreeMapAdditionalPropertyType8,
    )
    from ..models.b_tree_map_additional_property_type_9 import (
        BTreeMapAdditionalPropertyType9,
    )
    from ..models.b_tree_map_additional_property_type_10 import (
        BTreeMapAdditionalPropertyType10,
    )
    from ..models.b_tree_map_additional_property_type_11 import (
        BTreeMapAdditionalPropertyType11,
    )
    from ..models.b_tree_map_additional_property_type_12 import (
        BTreeMapAdditionalPropertyType12,
    )
    from ..models.b_tree_map_additional_property_type_13 import (
        BTreeMapAdditionalPropertyType13,
    )
    from ..models.b_tree_map_additional_property_type_14 import (
        BTreeMapAdditionalPropertyType14,
    )
    from ..models.b_tree_map_additional_property_type_15 import (
        BTreeMapAdditionalPropertyType15,
    )
    from ..models.b_tree_map_additional_property_type_16 import (
        BTreeMapAdditionalPropertyType16,
    )
    from ..models.b_tree_map_additional_property_type_17 import (
        BTreeMapAdditionalPropertyType17,
    )
    from ..models.b_tree_map_additional_property_type_18 import (
        BTreeMapAdditionalPropertyType18,
    )


T = TypeVar("T", bound="BTreeMap")


@_attrs_define
class BTreeMap:
    """ """

    additional_properties: dict[
        str,
        BTreeMapAdditionalPropertyType0
        | BTreeMapAdditionalPropertyType1
        | BTreeMapAdditionalPropertyType10
        | BTreeMapAdditionalPropertyType11
        | BTreeMapAdditionalPropertyType12
        | BTreeMapAdditionalPropertyType13
        | BTreeMapAdditionalPropertyType14
        | BTreeMapAdditionalPropertyType15
        | BTreeMapAdditionalPropertyType16
        | BTreeMapAdditionalPropertyType17
        | BTreeMapAdditionalPropertyType18
        | BTreeMapAdditionalPropertyType2
        | BTreeMapAdditionalPropertyType3
        | BTreeMapAdditionalPropertyType4
        | BTreeMapAdditionalPropertyType5
        | BTreeMapAdditionalPropertyType6
        | BTreeMapAdditionalPropertyType7
        | BTreeMapAdditionalPropertyType8
        | BTreeMapAdditionalPropertyType9,
    ] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.b_tree_map_additional_property_type_0 import (
            BTreeMapAdditionalPropertyType0,
        )
        from ..models.b_tree_map_additional_property_type_1 import (
            BTreeMapAdditionalPropertyType1,
        )
        from ..models.b_tree_map_additional_property_type_2 import (
            BTreeMapAdditionalPropertyType2,
        )
        from ..models.b_tree_map_additional_property_type_3 import (
            BTreeMapAdditionalPropertyType3,
        )
        from ..models.b_tree_map_additional_property_type_4 import (
            BTreeMapAdditionalPropertyType4,
        )
        from ..models.b_tree_map_additional_property_type_5 import (
            BTreeMapAdditionalPropertyType5,
        )
        from ..models.b_tree_map_additional_property_type_6 import (
            BTreeMapAdditionalPropertyType6,
        )
        from ..models.b_tree_map_additional_property_type_7 import (
            BTreeMapAdditionalPropertyType7,
        )
        from ..models.b_tree_map_additional_property_type_8 import (
            BTreeMapAdditionalPropertyType8,
        )
        from ..models.b_tree_map_additional_property_type_9 import (
            BTreeMapAdditionalPropertyType9,
        )
        from ..models.b_tree_map_additional_property_type_10 import (
            BTreeMapAdditionalPropertyType10,
        )
        from ..models.b_tree_map_additional_property_type_11 import (
            BTreeMapAdditionalPropertyType11,
        )
        from ..models.b_tree_map_additional_property_type_12 import (
            BTreeMapAdditionalPropertyType12,
        )
        from ..models.b_tree_map_additional_property_type_13 import (
            BTreeMapAdditionalPropertyType13,
        )
        from ..models.b_tree_map_additional_property_type_14 import (
            BTreeMapAdditionalPropertyType14,
        )
        from ..models.b_tree_map_additional_property_type_15 import (
            BTreeMapAdditionalPropertyType15,
        )
        from ..models.b_tree_map_additional_property_type_16 import (
            BTreeMapAdditionalPropertyType16,
        )
        from ..models.b_tree_map_additional_property_type_17 import (
            BTreeMapAdditionalPropertyType17,
        )

        field_dict: dict[str, Any] = {}
        for prop_name, prop in self.additional_properties.items():
            if isinstance(prop, BTreeMapAdditionalPropertyType0):
                field_dict[prop_name] = prop.to_dict()
            elif isinstance(prop, BTreeMapAdditionalPropertyType1):
                field_dict[prop_name] = prop.to_dict()
            elif isinstance(prop, BTreeMapAdditionalPropertyType2):
                field_dict[prop_name] = prop.to_dict()
            elif isinstance(prop, BTreeMapAdditionalPropertyType3):
                field_dict[prop_name] = prop.to_dict()
            elif isinstance(prop, BTreeMapAdditionalPropertyType4):
                field_dict[prop_name] = prop.to_dict()
            elif isinstance(prop, BTreeMapAdditionalPropertyType5):
                field_dict[prop_name] = prop.to_dict()
            elif isinstance(prop, BTreeMapAdditionalPropertyType6):
                field_dict[prop_name] = prop.to_dict()
            elif isinstance(prop, BTreeMapAdditionalPropertyType7):
                field_dict[prop_name] = prop.to_dict()
            elif isinstance(prop, BTreeMapAdditionalPropertyType8):
                field_dict[prop_name] = prop.to_dict()
            elif isinstance(prop, BTreeMapAdditionalPropertyType9):
                field_dict[prop_name] = prop.to_dict()
            elif isinstance(prop, BTreeMapAdditionalPropertyType10):
                field_dict[prop_name] = prop.to_dict()
            elif isinstance(prop, BTreeMapAdditionalPropertyType11):
                field_dict[prop_name] = prop.to_dict()
            elif isinstance(prop, BTreeMapAdditionalPropertyType12):
                field_dict[prop_name] = prop.to_dict()
            elif isinstance(prop, BTreeMapAdditionalPropertyType13):
                field_dict[prop_name] = prop.to_dict()
            elif isinstance(prop, BTreeMapAdditionalPropertyType14):
                field_dict[prop_name] = prop.to_dict()
            elif isinstance(prop, BTreeMapAdditionalPropertyType15):
                field_dict[prop_name] = prop.to_dict()
            elif isinstance(prop, BTreeMapAdditionalPropertyType16):
                field_dict[prop_name] = prop.to_dict()
            elif isinstance(prop, BTreeMapAdditionalPropertyType17):
                field_dict[prop_name] = prop.to_dict()
            else:
                field_dict[prop_name] = prop.to_dict()

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.b_tree_map_additional_property_type_0 import (
            BTreeMapAdditionalPropertyType0,
        )
        from ..models.b_tree_map_additional_property_type_1 import (
            BTreeMapAdditionalPropertyType1,
        )
        from ..models.b_tree_map_additional_property_type_2 import (
            BTreeMapAdditionalPropertyType2,
        )
        from ..models.b_tree_map_additional_property_type_3 import (
            BTreeMapAdditionalPropertyType3,
        )
        from ..models.b_tree_map_additional_property_type_4 import (
            BTreeMapAdditionalPropertyType4,
        )
        from ..models.b_tree_map_additional_property_type_5 import (
            BTreeMapAdditionalPropertyType5,
        )
        from ..models.b_tree_map_additional_property_type_6 import (
            BTreeMapAdditionalPropertyType6,
        )
        from ..models.b_tree_map_additional_property_type_7 import (
            BTreeMapAdditionalPropertyType7,
        )
        from ..models.b_tree_map_additional_property_type_8 import (
            BTreeMapAdditionalPropertyType8,
        )
        from ..models.b_tree_map_additional_property_type_9 import (
            BTreeMapAdditionalPropertyType9,
        )
        from ..models.b_tree_map_additional_property_type_10 import (
            BTreeMapAdditionalPropertyType10,
        )
        from ..models.b_tree_map_additional_property_type_11 import (
            BTreeMapAdditionalPropertyType11,
        )
        from ..models.b_tree_map_additional_property_type_12 import (
            BTreeMapAdditionalPropertyType12,
        )
        from ..models.b_tree_map_additional_property_type_13 import (
            BTreeMapAdditionalPropertyType13,
        )
        from ..models.b_tree_map_additional_property_type_14 import (
            BTreeMapAdditionalPropertyType14,
        )
        from ..models.b_tree_map_additional_property_type_15 import (
            BTreeMapAdditionalPropertyType15,
        )
        from ..models.b_tree_map_additional_property_type_16 import (
            BTreeMapAdditionalPropertyType16,
        )
        from ..models.b_tree_map_additional_property_type_17 import (
            BTreeMapAdditionalPropertyType17,
        )
        from ..models.b_tree_map_additional_property_type_18 import (
            BTreeMapAdditionalPropertyType18,
        )

        d = dict(src_dict)
        b_tree_map = cls()

        additional_properties = {}
        for prop_name, prop_dict in d.items():

            def _parse_additional_property(
                data: object,
            ) -> (
                BTreeMapAdditionalPropertyType0
                | BTreeMapAdditionalPropertyType1
                | BTreeMapAdditionalPropertyType10
                | BTreeMapAdditionalPropertyType11
                | BTreeMapAdditionalPropertyType12
                | BTreeMapAdditionalPropertyType13
                | BTreeMapAdditionalPropertyType14
                | BTreeMapAdditionalPropertyType15
                | BTreeMapAdditionalPropertyType16
                | BTreeMapAdditionalPropertyType17
                | BTreeMapAdditionalPropertyType18
                | BTreeMapAdditionalPropertyType2
                | BTreeMapAdditionalPropertyType3
                | BTreeMapAdditionalPropertyType4
                | BTreeMapAdditionalPropertyType5
                | BTreeMapAdditionalPropertyType6
                | BTreeMapAdditionalPropertyType7
                | BTreeMapAdditionalPropertyType8
                | BTreeMapAdditionalPropertyType9
            ):
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    additional_property_type_0 = (
                        BTreeMapAdditionalPropertyType0.from_dict(data)
                    )

                    return additional_property_type_0
                except (TypeError, ValueError, AttributeError, KeyError):
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    additional_property_type_1 = (
                        BTreeMapAdditionalPropertyType1.from_dict(data)
                    )

                    return additional_property_type_1
                except (TypeError, ValueError, AttributeError, KeyError):
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    additional_property_type_2 = (
                        BTreeMapAdditionalPropertyType2.from_dict(data)
                    )

                    return additional_property_type_2
                except (TypeError, ValueError, AttributeError, KeyError):
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    additional_property_type_3 = (
                        BTreeMapAdditionalPropertyType3.from_dict(data)
                    )

                    return additional_property_type_3
                except (TypeError, ValueError, AttributeError, KeyError):
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    additional_property_type_4 = (
                        BTreeMapAdditionalPropertyType4.from_dict(data)
                    )

                    return additional_property_type_4
                except (TypeError, ValueError, AttributeError, KeyError):
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    additional_property_type_5 = (
                        BTreeMapAdditionalPropertyType5.from_dict(data)
                    )

                    return additional_property_type_5
                except (TypeError, ValueError, AttributeError, KeyError):
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    additional_property_type_6 = (
                        BTreeMapAdditionalPropertyType6.from_dict(data)
                    )

                    return additional_property_type_6
                except (TypeError, ValueError, AttributeError, KeyError):
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    additional_property_type_7 = (
                        BTreeMapAdditionalPropertyType7.from_dict(data)
                    )

                    return additional_property_type_7
                except (TypeError, ValueError, AttributeError, KeyError):
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    additional_property_type_8 = (
                        BTreeMapAdditionalPropertyType8.from_dict(data)
                    )

                    return additional_property_type_8
                except (TypeError, ValueError, AttributeError, KeyError):
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    additional_property_type_9 = (
                        BTreeMapAdditionalPropertyType9.from_dict(data)
                    )

                    return additional_property_type_9
                except (TypeError, ValueError, AttributeError, KeyError):
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    additional_property_type_10 = (
                        BTreeMapAdditionalPropertyType10.from_dict(data)
                    )

                    return additional_property_type_10
                except (TypeError, ValueError, AttributeError, KeyError):
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    additional_property_type_11 = (
                        BTreeMapAdditionalPropertyType11.from_dict(data)
                    )

                    return additional_property_type_11
                except (TypeError, ValueError, AttributeError, KeyError):
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    additional_property_type_12 = (
                        BTreeMapAdditionalPropertyType12.from_dict(data)
                    )

                    return additional_property_type_12
                except (TypeError, ValueError, AttributeError, KeyError):
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    additional_property_type_13 = (
                        BTreeMapAdditionalPropertyType13.from_dict(data)
                    )

                    return additional_property_type_13
                except (TypeError, ValueError, AttributeError, KeyError):
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    additional_property_type_14 = (
                        BTreeMapAdditionalPropertyType14.from_dict(data)
                    )

                    return additional_property_type_14
                except (TypeError, ValueError, AttributeError, KeyError):
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    additional_property_type_15 = (
                        BTreeMapAdditionalPropertyType15.from_dict(data)
                    )

                    return additional_property_type_15
                except (TypeError, ValueError, AttributeError, KeyError):
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    additional_property_type_16 = (
                        BTreeMapAdditionalPropertyType16.from_dict(data)
                    )

                    return additional_property_type_16
                except (TypeError, ValueError, AttributeError, KeyError):
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    additional_property_type_17 = (
                        BTreeMapAdditionalPropertyType17.from_dict(data)
                    )

                    return additional_property_type_17
                except (TypeError, ValueError, AttributeError, KeyError):
                    pass
                if not isinstance(data, dict):
                    raise TypeError()
                additional_property_type_18 = (
                    BTreeMapAdditionalPropertyType18.from_dict(data)
                )

                return additional_property_type_18

            additional_property = _parse_additional_property(prop_dict)

            additional_properties[prop_name] = additional_property

        b_tree_map.additional_properties = additional_properties
        return b_tree_map

    @property
    def additional_keys(self) -> list[str]:
        return list(self.additional_properties.keys())

    def __getitem__(
        self, key: str
    ) -> (
        BTreeMapAdditionalPropertyType0
        | BTreeMapAdditionalPropertyType1
        | BTreeMapAdditionalPropertyType10
        | BTreeMapAdditionalPropertyType11
        | BTreeMapAdditionalPropertyType12
        | BTreeMapAdditionalPropertyType13
        | BTreeMapAdditionalPropertyType14
        | BTreeMapAdditionalPropertyType15
        | BTreeMapAdditionalPropertyType16
        | BTreeMapAdditionalPropertyType17
        | BTreeMapAdditionalPropertyType18
        | BTreeMapAdditionalPropertyType2
        | BTreeMapAdditionalPropertyType3
        | BTreeMapAdditionalPropertyType4
        | BTreeMapAdditionalPropertyType5
        | BTreeMapAdditionalPropertyType6
        | BTreeMapAdditionalPropertyType7
        | BTreeMapAdditionalPropertyType8
        | BTreeMapAdditionalPropertyType9
    ):
        return self.additional_properties[key]

    def __setitem__(
        self,
        key: str,
        value: BTreeMapAdditionalPropertyType0
        | BTreeMapAdditionalPropertyType1
        | BTreeMapAdditionalPropertyType10
        | BTreeMapAdditionalPropertyType11
        | BTreeMapAdditionalPropertyType12
        | BTreeMapAdditionalPropertyType13
        | BTreeMapAdditionalPropertyType14
        | BTreeMapAdditionalPropertyType15
        | BTreeMapAdditionalPropertyType16
        | BTreeMapAdditionalPropertyType17
        | BTreeMapAdditionalPropertyType18
        | BTreeMapAdditionalPropertyType2
        | BTreeMapAdditionalPropertyType3
        | BTreeMapAdditionalPropertyType4
        | BTreeMapAdditionalPropertyType5
        | BTreeMapAdditionalPropertyType6
        | BTreeMapAdditionalPropertyType7
        | BTreeMapAdditionalPropertyType8
        | BTreeMapAdditionalPropertyType9,
    ) -> None:
        self.additional_properties[key] = value

    def __delitem__(self, key: str) -> None:
        del self.additional_properties[key]

    def __contains__(self, key: str) -> bool:
        return key in self.additional_properties
