from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="IdentifyAttachmentRequest")


@_attrs_define
class IdentifyAttachmentRequest:
    """Request body for
    `POST /api/v1/devices/{id}/attachments/{slot}/identify`.

    Carries the base identify parameters plus the selectors that narrow
    the blink to one attached component instance.

        Attributes:
            color (None | str | Unset):
            duration_ms (int | None | Unset):
            binding_index (int | None | Unset):
            instance (int | None | Unset):
    """

    color: None | str | Unset = UNSET
    duration_ms: int | None | Unset = UNSET
    binding_index: int | None | Unset = UNSET
    instance: int | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        color: None | str | Unset
        if isinstance(self.color, Unset):
            color = UNSET
        else:
            color = self.color

        duration_ms: int | None | Unset
        if isinstance(self.duration_ms, Unset):
            duration_ms = UNSET
        else:
            duration_ms = self.duration_ms

        binding_index: int | None | Unset
        if isinstance(self.binding_index, Unset):
            binding_index = UNSET
        else:
            binding_index = self.binding_index

        instance: int | None | Unset
        if isinstance(self.instance, Unset):
            instance = UNSET
        else:
            instance = self.instance

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if color is not UNSET:
            field_dict["color"] = color
        if duration_ms is not UNSET:
            field_dict["duration_ms"] = duration_ms
        if binding_index is not UNSET:
            field_dict["binding_index"] = binding_index
        if instance is not UNSET:
            field_dict["instance"] = instance

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)

        def _parse_color(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        color = _parse_color(d.pop("color", UNSET))

        def _parse_duration_ms(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        duration_ms = _parse_duration_ms(d.pop("duration_ms", UNSET))

        def _parse_binding_index(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        binding_index = _parse_binding_index(d.pop("binding_index", UNSET))

        def _parse_instance(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        instance = _parse_instance(d.pop("instance", UNSET))

        identify_attachment_request = cls(
            color=color,
            duration_ms=duration_ms,
            binding_index=binding_index,
            instance=instance,
        )

        identify_attachment_request.additional_properties = d
        return identify_attachment_request

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
