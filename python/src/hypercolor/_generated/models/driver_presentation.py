from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.device_class_hint import DeviceClassHint
from ..types import UNSET, Unset

T = TypeVar("T", bound="DriverPresentation")


@_attrs_define
class DriverPresentation:
    """API and UI presentation metadata for a driver module.

    Attributes:
        label (str): Human-readable driver label.
        accent_rgb (list[int] | None | Unset): Primary RGB accent color.
        default_device_class (DeviceClassHint | None | Unset):
        icon (None | str | Unset): Stable icon identifier.
        secondary_rgb (list[int] | None | Unset): Secondary RGB accent color.
        short_label (None | str | Unset): Compact label for dense UI surfaces.
    """

    label: str
    accent_rgb: list[int] | None | Unset = UNSET
    default_device_class: DeviceClassHint | None | Unset = UNSET
    icon: None | str | Unset = UNSET
    secondary_rgb: list[int] | None | Unset = UNSET
    short_label: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        label = self.label

        accent_rgb: list[int] | None | Unset
        if isinstance(self.accent_rgb, Unset):
            accent_rgb = UNSET
        elif isinstance(self.accent_rgb, list):
            accent_rgb = self.accent_rgb

        else:
            accent_rgb = self.accent_rgb

        default_device_class: None | str | Unset
        if isinstance(self.default_device_class, Unset):
            default_device_class = UNSET
        elif isinstance(self.default_device_class, DeviceClassHint):
            default_device_class = self.default_device_class.value
        else:
            default_device_class = self.default_device_class

        icon: None | str | Unset
        if isinstance(self.icon, Unset):
            icon = UNSET
        else:
            icon = self.icon

        secondary_rgb: list[int] | None | Unset
        if isinstance(self.secondary_rgb, Unset):
            secondary_rgb = UNSET
        elif isinstance(self.secondary_rgb, list):
            secondary_rgb = self.secondary_rgb

        else:
            secondary_rgb = self.secondary_rgb

        short_label: None | str | Unset
        if isinstance(self.short_label, Unset):
            short_label = UNSET
        else:
            short_label = self.short_label

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "label": label,
            }
        )
        if accent_rgb is not UNSET:
            field_dict["accent_rgb"] = accent_rgb
        if default_device_class is not UNSET:
            field_dict["default_device_class"] = default_device_class
        if icon is not UNSET:
            field_dict["icon"] = icon
        if secondary_rgb is not UNSET:
            field_dict["secondary_rgb"] = secondary_rgb
        if short_label is not UNSET:
            field_dict["short_label"] = short_label

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        label = d.pop("label")

        def _parse_accent_rgb(data: object) -> list[int] | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, list):
                    raise TypeError()
                accent_rgb_type_0 = cast(list[int], data)

                return accent_rgb_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(list[int] | None | Unset, data)

        accent_rgb = _parse_accent_rgb(d.pop("accent_rgb", UNSET))

        def _parse_default_device_class(data: object) -> DeviceClassHint | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                default_device_class_type_1 = DeviceClassHint(data)

                return default_device_class_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(DeviceClassHint | None | Unset, data)

        default_device_class = _parse_default_device_class(
            d.pop("default_device_class", UNSET)
        )

        def _parse_icon(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        icon = _parse_icon(d.pop("icon", UNSET))

        def _parse_secondary_rgb(data: object) -> list[int] | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, list):
                    raise TypeError()
                secondary_rgb_type_0 = cast(list[int], data)

                return secondary_rgb_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(list[int] | None | Unset, data)

        secondary_rgb = _parse_secondary_rgb(d.pop("secondary_rgb", UNSET))

        def _parse_short_label(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        short_label = _parse_short_label(d.pop("short_label", UNSET))

        driver_presentation = cls(
            label=label,
            accent_rgb=accent_rgb,
            default_device_class=default_device_class,
            icon=icon,
            secondary_rgb=secondary_rgb,
            short_label=short_label,
        )

        driver_presentation.additional_properties = d
        return driver_presentation

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
