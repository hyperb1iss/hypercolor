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
from ..models.component_origin import ComponentOrigin
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.component_category_type_10 import ComponentCategoryType10


T = TypeVar("T", bound="TemplateSummary")


@_attrs_define
class TemplateSummary:
    """One template in the catalog listing.

    `led_count` is the template's resolved LED total, derived from its
    topology rather than stored, so it is always present even for
    templates whose topology is generated.

        Attributes:
            category (ComponentCategoryType0 | ComponentCategoryType1 | ComponentCategoryType10 | ComponentCategoryType2 |
                ComponentCategoryType3 | ComponentCategoryType4 | ComponentCategoryType5 | ComponentCategoryType6 |
                ComponentCategoryType7 | ComponentCategoryType8 | ComponentCategoryType9): Template category used for filtering
                and UI grouping.
            description (str):
            id (str):
            led_count (int):
            name (str):
            origin (ComponentOrigin): Where an attachment template came from.
            vendor (str):
            image_url (None | str | Unset):
            tags (list[str] | Unset):
    """

    category: (
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
    )
    description: str
    id: str
    led_count: int
    name: str
    origin: ComponentOrigin
    vendor: str
    image_url: None | str | Unset = UNSET
    tags: list[str] | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        category: dict[str, Any] | str
        if isinstance(self.category, ComponentCategoryType0):
            category = self.category.value
        elif isinstance(self.category, ComponentCategoryType1):
            category = self.category.value
        elif isinstance(self.category, ComponentCategoryType2):
            category = self.category.value
        elif isinstance(self.category, ComponentCategoryType3):
            category = self.category.value
        elif isinstance(self.category, ComponentCategoryType4):
            category = self.category.value
        elif isinstance(self.category, ComponentCategoryType5):
            category = self.category.value
        elif isinstance(self.category, ComponentCategoryType6):
            category = self.category.value
        elif isinstance(self.category, ComponentCategoryType7):
            category = self.category.value
        elif isinstance(self.category, ComponentCategoryType8):
            category = self.category.value
        elif isinstance(self.category, ComponentCategoryType9):
            category = self.category.value
        else:
            category = self.category.to_dict()

        description = self.description

        id = self.id

        led_count = self.led_count

        name = self.name

        origin = self.origin.value

        vendor = self.vendor

        image_url: None | str | Unset
        if isinstance(self.image_url, Unset):
            image_url = UNSET
        else:
            image_url = self.image_url

        tags: list[str] | Unset = UNSET
        if not isinstance(self.tags, Unset):
            tags = self.tags

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "category": category,
                "description": description,
                "id": id,
                "led_count": led_count,
                "name": name,
                "origin": origin,
                "vendor": vendor,
            }
        )
        if image_url is not UNSET:
            field_dict["image_url"] = image_url
        if tags is not UNSET:
            field_dict["tags"] = tags

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.component_category_type_10 import ComponentCategoryType10

        d = dict(src_dict)

        def _parse_category(
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
                componentsschemas_component_category_type_0 = ComponentCategoryType0(
                    data
                )

                return componentsschemas_component_category_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, str):
                    raise TypeError()
                componentsschemas_component_category_type_1 = ComponentCategoryType1(
                    data
                )

                return componentsschemas_component_category_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, str):
                    raise TypeError()
                componentsschemas_component_category_type_2 = ComponentCategoryType2(
                    data
                )

                return componentsschemas_component_category_type_2
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, str):
                    raise TypeError()
                componentsschemas_component_category_type_3 = ComponentCategoryType3(
                    data
                )

                return componentsschemas_component_category_type_3
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, str):
                    raise TypeError()
                componentsschemas_component_category_type_4 = ComponentCategoryType4(
                    data
                )

                return componentsschemas_component_category_type_4
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, str):
                    raise TypeError()
                componentsschemas_component_category_type_5 = ComponentCategoryType5(
                    data
                )

                return componentsschemas_component_category_type_5
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, str):
                    raise TypeError()
                componentsschemas_component_category_type_6 = ComponentCategoryType6(
                    data
                )

                return componentsschemas_component_category_type_6
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, str):
                    raise TypeError()
                componentsschemas_component_category_type_7 = ComponentCategoryType7(
                    data
                )

                return componentsschemas_component_category_type_7
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, str):
                    raise TypeError()
                componentsschemas_component_category_type_8 = ComponentCategoryType8(
                    data
                )

                return componentsschemas_component_category_type_8
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, str):
                    raise TypeError()
                componentsschemas_component_category_type_9 = ComponentCategoryType9(
                    data
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

        category = _parse_category(d.pop("category"))

        description = d.pop("description")

        id = d.pop("id")

        led_count = d.pop("led_count")

        name = d.pop("name")

        origin = ComponentOrigin(d.pop("origin"))

        vendor = d.pop("vendor")

        def _parse_image_url(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        image_url = _parse_image_url(d.pop("image_url", UNSET))

        tags = cast(list[str], d.pop("tags", UNSET))

        template_summary = cls(
            category=category,
            description=description,
            id=id,
            led_count=led_count,
            name=name,
            origin=origin,
            vendor=vendor,
            image_url=image_url,
            tags=tags,
        )

        template_summary.additional_properties = d
        return template_summary

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
