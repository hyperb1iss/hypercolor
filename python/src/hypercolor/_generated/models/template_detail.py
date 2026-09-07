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
    from ..models.component_canvas_size import ComponentCanvasSize
    from ..models.component_category_type_10 import ComponentCategoryType10
    from ..models.component_compatibility import ComponentCompatibility
    from ..models.led_topology_type_0 import LedTopologyType0
    from ..models.led_topology_type_1 import LedTopologyType1
    from ..models.led_topology_type_2 import LedTopologyType2
    from ..models.led_topology_type_3 import LedTopologyType3
    from ..models.led_topology_type_4 import LedTopologyType4
    from ..models.led_topology_type_5 import LedTopologyType5
    from ..models.led_topology_type_6 import LedTopologyType6
    from ..models.normalized_position import NormalizedPosition


T = TypeVar("T", bound="TemplateDetail")


@_attrs_define
class TemplateDetail:
    """Response for `POST /api/v1/attachments/templates`, the created
    template.

    The summary's fields plus everything needed to place the attachment:
    `led_positions` is expanded from the topology at request time, so it
    is present here but never in the listing.

    The item routes that also return this body today (`GET` and `PUT` on
    `/attachments/templates/{id}`) are deleted in wave 78.5, which leaves
    creation as the only caller. The type is chartered on creation for
    that reason, and the collection listing keeps its own summary shape.

        Attributes:
            category (ComponentCategoryType0 | ComponentCategoryType1 | ComponentCategoryType10 | ComponentCategoryType2 |
                ComponentCategoryType3 | ComponentCategoryType4 | ComponentCategoryType5 | ComponentCategoryType6 |
                ComponentCategoryType7 | ComponentCategoryType8 | ComponentCategoryType9): Template category used for filtering
                and UI grouping.
            default_size (ComponentCanvasSize): Default visual footprint for placing an attachment in the layout editor.
            description (str):
            id (str):
            led_count (int):
            name (str):
            origin (ComponentOrigin): Where an attachment template came from.
            topology (LedTopologyType0 | LedTopologyType1 | LedTopologyType2 | LedTopologyType3 | LedTopologyType4 |
                LedTopologyType5 | LedTopologyType6): LED arrangement within a zone's bounding rectangle.

                Each variant computes zone-local positions in normalized `[0.0, 1.0]` space.
                The topology determines how many LEDs exist and where they sit within
                the zone's rectangular bounds.
            vendor (str):
            compatible_slots (list[ComponentCompatibility] | Unset):
            image_url (None | str | Unset):
            led_mapping (list[int] | None | Unset):
            led_names (list[str] | None | Unset):
            led_positions (list[NormalizedPosition] | Unset):
            physical_size_mm (list[float] | None | Unset): Physical footprint in millimeters, as `[width, height]`.
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
    default_size: ComponentCanvasSize
    description: str
    id: str
    led_count: int
    name: str
    origin: ComponentOrigin
    topology: (
        LedTopologyType0
        | LedTopologyType1
        | LedTopologyType2
        | LedTopologyType3
        | LedTopologyType4
        | LedTopologyType5
        | LedTopologyType6
    )
    vendor: str
    compatible_slots: list[ComponentCompatibility] | Unset = UNSET
    image_url: None | str | Unset = UNSET
    led_mapping: list[int] | None | Unset = UNSET
    led_names: list[str] | None | Unset = UNSET
    led_positions: list[NormalizedPosition] | Unset = UNSET
    physical_size_mm: list[float] | None | Unset = UNSET
    tags: list[str] | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.led_topology_type_0 import LedTopologyType0
        from ..models.led_topology_type_1 import LedTopologyType1
        from ..models.led_topology_type_2 import LedTopologyType2
        from ..models.led_topology_type_3 import LedTopologyType3
        from ..models.led_topology_type_4 import LedTopologyType4
        from ..models.led_topology_type_5 import LedTopologyType5

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

        default_size = self.default_size.to_dict()

        description = self.description

        id = self.id

        led_count = self.led_count

        name = self.name

        origin = self.origin.value

        topology: dict[str, Any]
        if isinstance(self.topology, LedTopologyType0):
            topology = self.topology.to_dict()
        elif isinstance(self.topology, LedTopologyType1):
            topology = self.topology.to_dict()
        elif isinstance(self.topology, LedTopologyType2):
            topology = self.topology.to_dict()
        elif isinstance(self.topology, LedTopologyType3):
            topology = self.topology.to_dict()
        elif isinstance(self.topology, LedTopologyType4):
            topology = self.topology.to_dict()
        elif isinstance(self.topology, LedTopologyType5):
            topology = self.topology.to_dict()
        else:
            topology = self.topology.to_dict()

        vendor = self.vendor

        compatible_slots: list[dict[str, Any]] | Unset = UNSET
        if not isinstance(self.compatible_slots, Unset):
            compatible_slots = []
            for compatible_slots_item_data in self.compatible_slots:
                compatible_slots_item = compatible_slots_item_data.to_dict()
                compatible_slots.append(compatible_slots_item)

        image_url: None | str | Unset
        if isinstance(self.image_url, Unset):
            image_url = UNSET
        else:
            image_url = self.image_url

        led_mapping: list[int] | None | Unset
        if isinstance(self.led_mapping, Unset):
            led_mapping = UNSET
        elif isinstance(self.led_mapping, list):
            led_mapping = self.led_mapping

        else:
            led_mapping = self.led_mapping

        led_names: list[str] | None | Unset
        if isinstance(self.led_names, Unset):
            led_names = UNSET
        elif isinstance(self.led_names, list):
            led_names = self.led_names

        else:
            led_names = self.led_names

        led_positions: list[dict[str, Any]] | Unset = UNSET
        if not isinstance(self.led_positions, Unset):
            led_positions = []
            for led_positions_item_data in self.led_positions:
                led_positions_item = led_positions_item_data.to_dict()
                led_positions.append(led_positions_item)

        physical_size_mm: list[float] | None | Unset
        if isinstance(self.physical_size_mm, Unset):
            physical_size_mm = UNSET
        elif isinstance(self.physical_size_mm, list):
            physical_size_mm = self.physical_size_mm

        else:
            physical_size_mm = self.physical_size_mm

        tags: list[str] | Unset = UNSET
        if not isinstance(self.tags, Unset):
            tags = self.tags

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "category": category,
                "default_size": default_size,
                "description": description,
                "id": id,
                "led_count": led_count,
                "name": name,
                "origin": origin,
                "topology": topology,
                "vendor": vendor,
            }
        )
        if compatible_slots is not UNSET:
            field_dict["compatible_slots"] = compatible_slots
        if image_url is not UNSET:
            field_dict["image_url"] = image_url
        if led_mapping is not UNSET:
            field_dict["led_mapping"] = led_mapping
        if led_names is not UNSET:
            field_dict["led_names"] = led_names
        if led_positions is not UNSET:
            field_dict["led_positions"] = led_positions
        if physical_size_mm is not UNSET:
            field_dict["physical_size_mm"] = physical_size_mm
        if tags is not UNSET:
            field_dict["tags"] = tags

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.component_canvas_size import ComponentCanvasSize
        from ..models.component_category_type_10 import ComponentCategoryType10
        from ..models.component_compatibility import ComponentCompatibility
        from ..models.led_topology_type_0 import LedTopologyType0
        from ..models.led_topology_type_1 import LedTopologyType1
        from ..models.led_topology_type_2 import LedTopologyType2
        from ..models.led_topology_type_3 import LedTopologyType3
        from ..models.led_topology_type_4 import LedTopologyType4
        from ..models.led_topology_type_5 import LedTopologyType5
        from ..models.led_topology_type_6 import LedTopologyType6
        from ..models.normalized_position import NormalizedPosition

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

        default_size = ComponentCanvasSize.from_dict(d.pop("default_size"))

        description = d.pop("description")

        id = d.pop("id")

        led_count = d.pop("led_count")

        name = d.pop("name")

        origin = ComponentOrigin(d.pop("origin"))

        def _parse_topology(
            data: object,
        ) -> (
            LedTopologyType0
            | LedTopologyType1
            | LedTopologyType2
            | LedTopologyType3
            | LedTopologyType4
            | LedTopologyType5
            | LedTopologyType6
        ):
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_led_topology_type_0 = LedTopologyType0.from_dict(data)

                return componentsschemas_led_topology_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_led_topology_type_1 = LedTopologyType1.from_dict(data)

                return componentsschemas_led_topology_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_led_topology_type_2 = LedTopologyType2.from_dict(data)

                return componentsschemas_led_topology_type_2
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_led_topology_type_3 = LedTopologyType3.from_dict(data)

                return componentsschemas_led_topology_type_3
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_led_topology_type_4 = LedTopologyType4.from_dict(data)

                return componentsschemas_led_topology_type_4
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_led_topology_type_5 = LedTopologyType5.from_dict(data)

                return componentsschemas_led_topology_type_5
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            if not isinstance(data, dict):
                raise TypeError()
            componentsschemas_led_topology_type_6 = LedTopologyType6.from_dict(data)

            return componentsschemas_led_topology_type_6

        topology = _parse_topology(d.pop("topology"))

        vendor = d.pop("vendor")

        _compatible_slots = d.pop("compatible_slots", UNSET)
        compatible_slots: list[ComponentCompatibility] | Unset = UNSET
        if _compatible_slots is not UNSET:
            compatible_slots = []
            for compatible_slots_item_data in _compatible_slots:
                compatible_slots_item = ComponentCompatibility.from_dict(
                    compatible_slots_item_data
                )

                compatible_slots.append(compatible_slots_item)

        def _parse_image_url(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        image_url = _parse_image_url(d.pop("image_url", UNSET))

        def _parse_led_mapping(data: object) -> list[int] | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, list):
                    raise TypeError()
                led_mapping_type_0 = cast(list[int], data)

                return led_mapping_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(list[int] | None | Unset, data)

        led_mapping = _parse_led_mapping(d.pop("led_mapping", UNSET))

        def _parse_led_names(data: object) -> list[str] | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, list):
                    raise TypeError()
                led_names_type_0 = cast(list[str], data)

                return led_names_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(list[str] | None | Unset, data)

        led_names = _parse_led_names(d.pop("led_names", UNSET))

        _led_positions = d.pop("led_positions", UNSET)
        led_positions: list[NormalizedPosition] | Unset = UNSET
        if _led_positions is not UNSET:
            led_positions = []
            for led_positions_item_data in _led_positions:
                led_positions_item = NormalizedPosition.from_dict(
                    led_positions_item_data
                )

                led_positions.append(led_positions_item)

        def _parse_physical_size_mm(data: object) -> list[float] | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, list):
                    raise TypeError()
                physical_size_mm_type_0 = cast(list[float], data)

                return physical_size_mm_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(list[float] | None | Unset, data)

        physical_size_mm = _parse_physical_size_mm(d.pop("physical_size_mm", UNSET))

        tags = cast(list[str], d.pop("tags", UNSET))

        template_detail = cls(
            category=category,
            default_size=default_size,
            description=description,
            id=id,
            led_count=led_count,
            name=name,
            origin=origin,
            topology=topology,
            vendor=vendor,
            compatible_slots=compatible_slots,
            image_url=image_url,
            led_mapping=led_mapping,
            led_names=led_names,
            led_positions=led_positions,
            physical_size_mm=physical_size_mm,
            tags=tags,
        )

        template_detail.additional_properties = d
        return template_detail

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
