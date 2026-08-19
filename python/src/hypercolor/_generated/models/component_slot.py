from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.component_category_type_0 import ComponentCategoryType0
from ..models.component_category_type_1 import ComponentCategoryType1
from ..models.component_category_type_2 import ComponentCategoryType2
from ..models.component_category_type_3 import ComponentCategoryType3
from ..models.component_category_type_4 import ComponentCategoryType4
from ..models.component_category_type_5 import ComponentCategoryType5
from ..models.component_category_type_6 import ComponentCategoryType6
from ..models.component_category_type_7 import ComponentCategoryType7
from ..models.component_category_type_8 import ComponentCategoryType8
from ..models.component_category_type_9 import ComponentCategoryType9
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.component_category_type_10 import ComponentCategoryType10


T = TypeVar("T", bound="ComponentSlot")


@_attrs_define
class ComponentSlot:
    """One physical controller attachment point.

    Attributes:
        id (str): Stable slot identifier.
        led_count (int): Number of LEDs available to the slot.
        led_start (int): Inclusive LED start index on the physical controller.
        name (str): User-facing port/channel name.
        allow_custom (bool | Unset): Whether user-authored templates may be bound here.
        allowed_templates (list[str] | Unset): Explicit template IDs that should be offered regardless of category.
        suggested_categories (list[ComponentCategoryType0 | ComponentCategoryType1 | ComponentCategoryType10 |
            ComponentCategoryType2 | ComponentCategoryType3 | ComponentCategoryType4 | ComponentCategoryType5 |
            ComponentCategoryType6 | ComponentCategoryType7 | ComponentCategoryType8 | ComponentCategoryType9] | Unset):
            Template categories that make sense here.
    """

    id: str
    led_count: int
    led_start: int
    name: str
    allow_custom: bool | Unset = UNSET
    allowed_templates: list[str] | Unset = UNSET
    suggested_categories: (
        list[
            ComponentCategoryType0
            | ComponentCategoryType1
            | ComponentCategoryType10
            | ComponentCategoryType2
            | ComponentCategoryType3
            | ComponentCategoryType4
            | ComponentCategoryType5
            | ComponentCategoryType6
            | ComponentCategoryType7
            | ComponentCategoryType8
            | ComponentCategoryType9
        ]
        | Unset
    ) = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        id = self.id

        led_count = self.led_count

        led_start = self.led_start

        name = self.name

        allow_custom = self.allow_custom

        allowed_templates: list[str] | Unset = UNSET
        if not isinstance(self.allowed_templates, Unset):
            allowed_templates = self.allowed_templates

        suggested_categories: list[dict[str, Any] | str] | Unset = UNSET
        if not isinstance(self.suggested_categories, Unset):
            suggested_categories = []
            for suggested_categories_item_data in self.suggested_categories:
                suggested_categories_item: dict[str, Any] | str
                if isinstance(suggested_categories_item_data, ComponentCategoryType0):
                    suggested_categories_item = suggested_categories_item_data.value
                elif isinstance(suggested_categories_item_data, ComponentCategoryType1):
                    suggested_categories_item = suggested_categories_item_data.value
                elif isinstance(suggested_categories_item_data, ComponentCategoryType2):
                    suggested_categories_item = suggested_categories_item_data.value
                elif isinstance(suggested_categories_item_data, ComponentCategoryType3):
                    suggested_categories_item = suggested_categories_item_data.value
                elif isinstance(suggested_categories_item_data, ComponentCategoryType4):
                    suggested_categories_item = suggested_categories_item_data.value
                elif isinstance(suggested_categories_item_data, ComponentCategoryType5):
                    suggested_categories_item = suggested_categories_item_data.value
                elif isinstance(suggested_categories_item_data, ComponentCategoryType6):
                    suggested_categories_item = suggested_categories_item_data.value
                elif isinstance(suggested_categories_item_data, ComponentCategoryType7):
                    suggested_categories_item = suggested_categories_item_data.value
                elif isinstance(suggested_categories_item_data, ComponentCategoryType8):
                    suggested_categories_item = suggested_categories_item_data.value
                elif isinstance(suggested_categories_item_data, ComponentCategoryType9):
                    suggested_categories_item = suggested_categories_item_data.value
                else:
                    suggested_categories_item = suggested_categories_item_data.to_dict()

                suggested_categories.append(suggested_categories_item)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "id": id,
                "led_count": led_count,
                "led_start": led_start,
                "name": name,
            }
        )
        if allow_custom is not UNSET:
            field_dict["allow_custom"] = allow_custom
        if allowed_templates is not UNSET:
            field_dict["allowed_templates"] = allowed_templates
        if suggested_categories is not UNSET:
            field_dict["suggested_categories"] = suggested_categories

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.component_category_type_10 import ComponentCategoryType10

        d = dict(src_dict)
        id = d.pop("id")

        led_count = d.pop("led_count")

        led_start = d.pop("led_start")

        name = d.pop("name")

        allow_custom = d.pop("allow_custom", UNSET)

        allowed_templates = cast(list[str], d.pop("allowed_templates", UNSET))

        _suggested_categories = d.pop("suggested_categories", UNSET)
        suggested_categories: (
            list[
                ComponentCategoryType0
                | ComponentCategoryType1
                | ComponentCategoryType10
                | ComponentCategoryType2
                | ComponentCategoryType3
                | ComponentCategoryType4
                | ComponentCategoryType5
                | ComponentCategoryType6
                | ComponentCategoryType7
                | ComponentCategoryType8
                | ComponentCategoryType9
            ]
            | Unset
        ) = UNSET
        if _suggested_categories is not UNSET:
            suggested_categories = []
            for suggested_categories_item_data in _suggested_categories:

                def _parse_suggested_categories_item(
                    data: object,
                ) -> (
                    ComponentCategoryType0
                    | ComponentCategoryType1
                    | ComponentCategoryType10
                    | ComponentCategoryType2
                    | ComponentCategoryType3
                    | ComponentCategoryType4
                    | ComponentCategoryType5
                    | ComponentCategoryType6
                    | ComponentCategoryType7
                    | ComponentCategoryType8
                    | ComponentCategoryType9
                ):
                    try:
                        if not isinstance(data, str):
                            raise TypeError()
                        componentsschemas_component_category_type_0 = (
                            ComponentCategoryType0(data)
                        )

                        return componentsschemas_component_category_type_0
                    except (TypeError, ValueError, AttributeError, KeyError):
                        pass
                    try:
                        if not isinstance(data, str):
                            raise TypeError()
                        componentsschemas_component_category_type_1 = (
                            ComponentCategoryType1(data)
                        )

                        return componentsschemas_component_category_type_1
                    except (TypeError, ValueError, AttributeError, KeyError):
                        pass
                    try:
                        if not isinstance(data, str):
                            raise TypeError()
                        componentsschemas_component_category_type_2 = (
                            ComponentCategoryType2(data)
                        )

                        return componentsschemas_component_category_type_2
                    except (TypeError, ValueError, AttributeError, KeyError):
                        pass
                    try:
                        if not isinstance(data, str):
                            raise TypeError()
                        componentsschemas_component_category_type_3 = (
                            ComponentCategoryType3(data)
                        )

                        return componentsschemas_component_category_type_3
                    except (TypeError, ValueError, AttributeError, KeyError):
                        pass
                    try:
                        if not isinstance(data, str):
                            raise TypeError()
                        componentsschemas_component_category_type_4 = (
                            ComponentCategoryType4(data)
                        )

                        return componentsschemas_component_category_type_4
                    except (TypeError, ValueError, AttributeError, KeyError):
                        pass
                    try:
                        if not isinstance(data, str):
                            raise TypeError()
                        componentsschemas_component_category_type_5 = (
                            ComponentCategoryType5(data)
                        )

                        return componentsschemas_component_category_type_5
                    except (TypeError, ValueError, AttributeError, KeyError):
                        pass
                    try:
                        if not isinstance(data, str):
                            raise TypeError()
                        componentsschemas_component_category_type_6 = (
                            ComponentCategoryType6(data)
                        )

                        return componentsschemas_component_category_type_6
                    except (TypeError, ValueError, AttributeError, KeyError):
                        pass
                    try:
                        if not isinstance(data, str):
                            raise TypeError()
                        componentsschemas_component_category_type_7 = (
                            ComponentCategoryType7(data)
                        )

                        return componentsschemas_component_category_type_7
                    except (TypeError, ValueError, AttributeError, KeyError):
                        pass
                    try:
                        if not isinstance(data, str):
                            raise TypeError()
                        componentsschemas_component_category_type_8 = (
                            ComponentCategoryType8(data)
                        )

                        return componentsschemas_component_category_type_8
                    except (TypeError, ValueError, AttributeError, KeyError):
                        pass
                    try:
                        if not isinstance(data, str):
                            raise TypeError()
                        componentsschemas_component_category_type_9 = (
                            ComponentCategoryType9(data)
                        )

                        return componentsschemas_component_category_type_9
                    except (TypeError, ValueError, AttributeError, KeyError):
                        pass
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_component_category_type_10 = (
                        ComponentCategoryType10.from_dict(data)
                    )

                    return componentsschemas_component_category_type_10

                suggested_categories_item = _parse_suggested_categories_item(
                    suggested_categories_item_data
                )

                suggested_categories.append(suggested_categories_item)

        component_slot = cls(
            id=id,
            led_count=led_count,
            led_start=led_start,
            name=name,
            allow_custom=allow_custom,
            allowed_templates=allowed_templates,
            suggested_categories=suggested_categories,
        )

        component_slot.additional_properties = d
        return component_slot

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
