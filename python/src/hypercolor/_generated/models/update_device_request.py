from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.display_rotation import DisplayRotation
from ..types import UNSET, Unset

T = TypeVar("T", bound="UpdateDeviceRequest")


@_attrs_define
class UpdateDeviceRequest:
    """Request body for `PUT /api/v1/devices/{id}`.

    Attributes:
        brightness (int | None | Unset):
        display_rotation (DisplayRotation | None | Unset):
        enabled (bool | None | Unset):
        name (None | str | Unset):
    """

    brightness: int | None | Unset = UNSET
    display_rotation: DisplayRotation | None | Unset = UNSET
    enabled: bool | None | Unset = UNSET
    name: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        brightness: int | None | Unset
        if isinstance(self.brightness, Unset):
            brightness = UNSET
        else:
            brightness = self.brightness

        display_rotation: None | str | Unset
        if isinstance(self.display_rotation, Unset):
            display_rotation = UNSET
        elif isinstance(self.display_rotation, DisplayRotation):
            display_rotation = self.display_rotation.value
        else:
            display_rotation = self.display_rotation

        enabled: bool | None | Unset
        if isinstance(self.enabled, Unset):
            enabled = UNSET
        else:
            enabled = self.enabled

        name: None | str | Unset
        if isinstance(self.name, Unset):
            name = UNSET
        else:
            name = self.name

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if brightness is not UNSET:
            field_dict["brightness"] = brightness
        if display_rotation is not UNSET:
            field_dict["display_rotation"] = display_rotation
        if enabled is not UNSET:
            field_dict["enabled"] = enabled
        if name is not UNSET:
            field_dict["name"] = name

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)

        def _parse_brightness(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        brightness = _parse_brightness(d.pop("brightness", UNSET))

        def _parse_display_rotation(data: object) -> DisplayRotation | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                display_rotation_type_1 = DisplayRotation(data)

                return display_rotation_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(DisplayRotation | None | Unset, data)

        display_rotation = _parse_display_rotation(d.pop("display_rotation", UNSET))

        def _parse_enabled(data: object) -> bool | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(bool | None | Unset, data)

        enabled = _parse_enabled(d.pop("enabled", UNSET))

        def _parse_name(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        name = _parse_name(d.pop("name", UNSET))

        update_device_request = cls(
            brightness=brightness,
            display_rotation=display_rotation,
            enabled=enabled,
            name=name,
        )

        update_device_request.additional_properties = d
        return update_device_request

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
