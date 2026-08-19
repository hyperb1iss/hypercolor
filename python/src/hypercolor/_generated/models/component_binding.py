from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="ComponentBinding")


@_attrs_define
class ComponentBinding:
    """Binding from a controller slot to a chosen attachment template.

    Attributes:
        slot_id (str): Slot receiving the attachment.
        template_id (str): Template identifier selected for this slot.
        enabled (bool | Unset): Whether the binding is active.
        instances (int | Unset): Number of chained template instances bound to the slot.
        led_offset (int | Unset): LED offset within the slot where the binding begins.
        name (None | str | Unset): Optional user-facing override for the attachment name.
    """

    slot_id: str
    template_id: str
    enabled: bool | Unset = UNSET
    instances: int | Unset = UNSET
    led_offset: int | Unset = UNSET
    name: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        slot_id = self.slot_id

        template_id = self.template_id

        enabled = self.enabled

        instances = self.instances

        led_offset = self.led_offset

        name: None | str | Unset
        if isinstance(self.name, Unset):
            name = UNSET
        else:
            name = self.name

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "slot_id": slot_id,
                "template_id": template_id,
            }
        )
        if enabled is not UNSET:
            field_dict["enabled"] = enabled
        if instances is not UNSET:
            field_dict["instances"] = instances
        if led_offset is not UNSET:
            field_dict["led_offset"] = led_offset
        if name is not UNSET:
            field_dict["name"] = name

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        slot_id = d.pop("slot_id")

        template_id = d.pop("template_id")

        enabled = d.pop("enabled", UNSET)

        instances = d.pop("instances", UNSET)

        led_offset = d.pop("led_offset", UNSET)

        def _parse_name(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        name = _parse_name(d.pop("name", UNSET))

        component_binding = cls(
            slot_id=slot_id,
            template_id=template_id,
            enabled=enabled,
            instances=instances,
            led_offset=led_offset,
            name=name,
        )

        component_binding.additional_properties = d
        return component_binding

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
