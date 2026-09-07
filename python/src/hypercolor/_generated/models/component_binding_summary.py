from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="ComponentBindingSummary")


@_attrs_define
class ComponentBindingSummary:
    """One resolved attachment binding, with the template it instantiates and
    the LED range it occupies on the controller.

        Attributes:
            effective_led_count (int):
            enabled (bool):
            instances (int):
            led_offset (int):
            slot_id (str):
            template_id (str):
            template_name (str):
            name (None | str | Unset):
    """

    effective_led_count: int
    enabled: bool
    instances: int
    led_offset: int
    slot_id: str
    template_id: str
    template_name: str
    name: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        effective_led_count = self.effective_led_count

        enabled = self.enabled

        instances = self.instances

        led_offset = self.led_offset

        slot_id = self.slot_id

        template_id = self.template_id

        template_name = self.template_name

        name: None | str | Unset
        if isinstance(self.name, Unset):
            name = UNSET
        else:
            name = self.name

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "effective_led_count": effective_led_count,
                "enabled": enabled,
                "instances": instances,
                "led_offset": led_offset,
                "slot_id": slot_id,
                "template_id": template_id,
                "template_name": template_name,
            }
        )
        if name is not UNSET:
            field_dict["name"] = name

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        effective_led_count = d.pop("effective_led_count")

        enabled = d.pop("enabled")

        instances = d.pop("instances")

        led_offset = d.pop("led_offset")

        slot_id = d.pop("slot_id")

        template_id = d.pop("template_id")

        template_name = d.pop("template_name")

        def _parse_name(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        name = _parse_name(d.pop("name", UNSET))

        component_binding_summary = cls(
            effective_led_count=effective_led_count,
            enabled=enabled,
            instances=instances,
            led_offset=led_offset,
            slot_id=slot_id,
            template_id=template_id,
            template_name=template_name,
            name=name,
        )

        component_binding_summary.additional_properties = d
        return component_binding_summary

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
