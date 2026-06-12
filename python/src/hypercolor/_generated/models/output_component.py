from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="OutputComponent")


@_attrs_define
class OutputComponent:
    """Attachment metadata carried by imported layout zones.

    Attributes:
        slot_id (str): Source slot ID on the physical controller.
        template_id (str): Bound attachment template identifier.
        instance (int | Unset): Zero-based attachment instance index within the binding.
        led_count (int | None | Unset): Physical LED count reserved for this imported attachment zone.
        led_mapping (list[int] | None | Unset): Optional spatial-order -> physical-order LED remapping.
        led_start (int | None | Unset): Inclusive physical LED start index for this imported attachment zone.
    """

    slot_id: str
    template_id: str
    instance: int | Unset = UNSET
    led_count: int | None | Unset = UNSET
    led_mapping: list[int] | None | Unset = UNSET
    led_start: int | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        slot_id = self.slot_id

        template_id = self.template_id

        instance = self.instance

        led_count: int | None | Unset
        if isinstance(self.led_count, Unset):
            led_count = UNSET
        else:
            led_count = self.led_count

        led_mapping: list[int] | None | Unset
        if isinstance(self.led_mapping, Unset):
            led_mapping = UNSET
        elif isinstance(self.led_mapping, list):
            led_mapping = self.led_mapping

        else:
            led_mapping = self.led_mapping

        led_start: int | None | Unset
        if isinstance(self.led_start, Unset):
            led_start = UNSET
        else:
            led_start = self.led_start

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "slot_id": slot_id,
                "template_id": template_id,
            }
        )
        if instance is not UNSET:
            field_dict["instance"] = instance
        if led_count is not UNSET:
            field_dict["led_count"] = led_count
        if led_mapping is not UNSET:
            field_dict["led_mapping"] = led_mapping
        if led_start is not UNSET:
            field_dict["led_start"] = led_start

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        slot_id = d.pop("slot_id")

        template_id = d.pop("template_id")

        instance = d.pop("instance", UNSET)

        def _parse_led_count(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        led_count = _parse_led_count(d.pop("led_count", UNSET))

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

        def _parse_led_start(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        led_start = _parse_led_start(d.pop("led_start", UNSET))

        output_component = cls(
            slot_id=slot_id,
            template_id=template_id,
            instance=instance,
            led_count=led_count,
            led_mapping=led_mapping,
            led_start=led_start,
        )

        output_component.additional_properties = d
        return output_component

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
