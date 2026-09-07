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
    from ..models.component_canvas_size import ComponentCanvasSize
    from ..models.component_category_type_10 import ComponentCategoryType10
    from ..models.led_topology_type_0 import LedTopologyType0
    from ..models.led_topology_type_1 import LedTopologyType1
    from ..models.led_topology_type_2 import LedTopologyType2
    from ..models.led_topology_type_3 import LedTopologyType3
    from ..models.led_topology_type_4 import LedTopologyType4
    from ..models.led_topology_type_5 import LedTopologyType5
    from ..models.led_topology_type_6 import LedTopologyType6


T = TypeVar("T", bound="ComponentSuggestedZone")


@_attrs_define
class ComponentSuggestedZone:
    """Attachment-derived zone suggestion for layout import and preview flows.

    Attributes:
        category (ComponentCategoryType0 | ComponentCategoryType1 | ComponentCategoryType10 | ComponentCategoryType2 |
            ComponentCategoryType3 | ComponentCategoryType4 | ComponentCategoryType5 | ComponentCategoryType6 |
            ComponentCategoryType7 | ComponentCategoryType8 | ComponentCategoryType9): Template category used for filtering
            and UI grouping.
        default_size (ComponentCanvasSize): Default visual footprint for placing an attachment in the layout editor.
        instance (int): Zero-based attachment instance index within the binding.
        led_count (int): Number of LEDs consumed by this instance.
        led_start (int): Inclusive LED start index on the physical controller.
        name (str): Final user-facing zone name for this attachment instance.
        slot_id (str): Source slot ID on the physical controller.
        template_id (str): Bound attachment template identifier.
        template_name (str): Bound attachment template display name.
        topology (LedTopologyType0 | LedTopologyType1 | LedTopologyType2 | LedTopologyType3 | LedTopologyType4 |
            LedTopologyType5 | LedTopologyType6): LED arrangement within a zone's bounding rectangle.

            Each variant computes zone-local positions in normalized `[0.0, 1.0]` space.
            The topology determines how many LEDs exist and where they sit within
            the zone's rectangular bounds.
        led_mapping (list[int] | None | Unset): Optional spatial-order -> physical-order LED remapping.
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
    instance: int
    led_count: int
    led_start: int
    name: str
    slot_id: str
    template_id: str
    template_name: str
    topology: (
        LedTopologyType0
        | LedTopologyType1
        | LedTopologyType2
        | LedTopologyType3
        | LedTopologyType4
        | LedTopologyType5
        | LedTopologyType6
    )
    led_mapping: list[int] | None | Unset = UNSET
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

        instance = self.instance

        led_count = self.led_count

        led_start = self.led_start

        name = self.name

        slot_id = self.slot_id

        template_id = self.template_id

        template_name = self.template_name

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

        led_mapping: list[int] | None | Unset
        if isinstance(self.led_mapping, Unset):
            led_mapping = UNSET
        elif isinstance(self.led_mapping, list):
            led_mapping = self.led_mapping

        else:
            led_mapping = self.led_mapping

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "category": category,
                "default_size": default_size,
                "instance": instance,
                "led_count": led_count,
                "led_start": led_start,
                "name": name,
                "slot_id": slot_id,
                "template_id": template_id,
                "template_name": template_name,
                "topology": topology,
            }
        )
        if led_mapping is not UNSET:
            field_dict["led_mapping"] = led_mapping

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.component_canvas_size import ComponentCanvasSize
        from ..models.component_category_type_10 import ComponentCategoryType10
        from ..models.led_topology_type_0 import LedTopologyType0
        from ..models.led_topology_type_1 import LedTopologyType1
        from ..models.led_topology_type_2 import LedTopologyType2
        from ..models.led_topology_type_3 import LedTopologyType3
        from ..models.led_topology_type_4 import LedTopologyType4
        from ..models.led_topology_type_5 import LedTopologyType5
        from ..models.led_topology_type_6 import LedTopologyType6

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

        instance = d.pop("instance")

        led_count = d.pop("led_count")

        led_start = d.pop("led_start")

        name = d.pop("name")

        slot_id = d.pop("slot_id")

        template_id = d.pop("template_id")

        template_name = d.pop("template_name")

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

        component_suggested_zone = cls(
            category=category,
            default_size=default_size,
            instance=instance,
            led_count=led_count,
            led_start=led_start,
            name=name,
            slot_id=slot_id,
            template_id=template_id,
            template_name=template_name,
            topology=topology,
            led_mapping=led_mapping,
        )

        component_suggested_zone.additional_properties = d
        return component_suggested_zone

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
