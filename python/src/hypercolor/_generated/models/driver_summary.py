from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.driver_module_descriptor import DriverModuleDescriptor


T = TypeVar("T", bound="DriverSummary")


@_attrs_define
class DriverSummary:
    """
    Attributes:
        config_key (str):
        descriptor (DriverModuleDescriptor): Stable module descriptor for native and future Wasm driver registries.
        enabled (bool):
    """

    config_key: str
    descriptor: DriverModuleDescriptor
    enabled: bool
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        config_key = self.config_key

        descriptor = self.descriptor.to_dict()

        enabled = self.enabled

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "config_key": config_key,
                "descriptor": descriptor,
                "enabled": enabled,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.driver_module_descriptor import DriverModuleDescriptor

        d = dict(src_dict)
        config_key = d.pop("config_key")

        descriptor = DriverModuleDescriptor.from_dict(d.pop("descriptor"))

        enabled = d.pop("enabled")

        driver_summary = cls(
            config_key=config_key,
            descriptor=descriptor,
            enabled=enabled,
        )

        driver_summary.additional_properties = d
        return driver_summary

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
