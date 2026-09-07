from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="IdentifyAttachmentResponse")


@_attrs_define
class IdentifyAttachmentResponse:
    """Response for
    `POST /api/v1/devices/{id}/attachments/{slot}/identify`.

    `instance` is `null` when the request blinked every instance of the
    binding rather than one of them.

        Attributes:
            binding_index (int):
            device_id (str):
            duration_ms (int):
            identifying (bool):
            slot_id (str):
            color (None | str | Unset):
            instance (int | None | Unset):
    """

    binding_index: int
    device_id: str
    duration_ms: int
    identifying: bool
    slot_id: str
    color: None | str | Unset = UNSET
    instance: int | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        binding_index = self.binding_index

        device_id = self.device_id

        duration_ms = self.duration_ms

        identifying = self.identifying

        slot_id = self.slot_id

        color: None | str | Unset
        if isinstance(self.color, Unset):
            color = UNSET
        else:
            color = self.color

        instance: int | None | Unset
        if isinstance(self.instance, Unset):
            instance = UNSET
        else:
            instance = self.instance

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "binding_index": binding_index,
                "device_id": device_id,
                "duration_ms": duration_ms,
                "identifying": identifying,
                "slot_id": slot_id,
            }
        )
        if color is not UNSET:
            field_dict["color"] = color
        if instance is not UNSET:
            field_dict["instance"] = instance

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        binding_index = d.pop("binding_index")

        device_id = d.pop("device_id")

        duration_ms = d.pop("duration_ms")

        identifying = d.pop("identifying")

        slot_id = d.pop("slot_id")

        def _parse_color(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        color = _parse_color(d.pop("color", UNSET))

        def _parse_instance(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        instance = _parse_instance(d.pop("instance", UNSET))

        identify_attachment_response = cls(
            binding_index=binding_index,
            device_id=device_id,
            duration_ms=duration_ms,
            identifying=identifying,
            slot_id=slot_id,
            color=color,
            instance=instance,
        )

        identify_attachment_response.additional_properties = d
        return identify_attachment_response

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
