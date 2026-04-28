from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.driver_module_descriptor import DriverModuleDescriptor
    from ..models.driver_protocol_descriptor import DriverProtocolDescriptor


T = TypeVar("T", bound="DriverSummary")


@_attrs_define
class DriverSummary:
    """
    Attributes:
        config_key (str):
        descriptor (DriverModuleDescriptor): Stable module descriptor for native and future Wasm driver registries.
        enabled (bool):
        control_surface_id (None | str | Unset):
        control_surface_path (None | str | Unset):
        protocols (list[DriverProtocolDescriptor]):
    """

    config_key: str
    descriptor: DriverModuleDescriptor
    enabled: bool
    control_surface_id: None | str | Unset = UNSET
    control_surface_path: None | str | Unset = UNSET
    protocols: list[DriverProtocolDescriptor] = _attrs_field(factory=list)
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        config_key = self.config_key

        descriptor = self.descriptor.to_dict()

        enabled = self.enabled

        protocols = []
        for protocols_item_data in self.protocols:
            protocols_item = protocols_item_data.to_dict()
            protocols.append(protocols_item)

        control_surface_id: None | str | Unset
        if isinstance(self.control_surface_id, Unset):
            control_surface_id = UNSET
        else:
            control_surface_id = self.control_surface_id

        control_surface_path: None | str | Unset
        if isinstance(self.control_surface_path, Unset):
            control_surface_path = UNSET
        else:
            control_surface_path = self.control_surface_path

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "config_key": config_key,
                "descriptor": descriptor,
                "enabled": enabled,
            }
        )
        if control_surface_id is not UNSET:
            field_dict["control_surface_id"] = control_surface_id
        if control_surface_path is not UNSET:
            field_dict["control_surface_path"] = control_surface_path
        if protocols:
            field_dict["protocols"] = protocols

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.driver_module_descriptor import DriverModuleDescriptor
        from ..models.driver_protocol_descriptor import DriverProtocolDescriptor

        d = dict(src_dict)
        config_key = d.pop("config_key")

        descriptor = DriverModuleDescriptor.from_dict(d.pop("descriptor"))

        enabled = d.pop("enabled")

        def _parse_control_surface_id(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        control_surface_id = _parse_control_surface_id(
            d.pop("control_surface_id", UNSET)
        )

        def _parse_control_surface_path(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        control_surface_path = _parse_control_surface_path(
            d.pop("control_surface_path", UNSET)
        )

        protocols = []
        _protocols = d.pop("protocols", [])
        for protocols_item_data in _protocols:
            protocols_item = DriverProtocolDescriptor.from_dict(protocols_item_data)
            protocols.append(protocols_item)

        driver_summary = cls(
            config_key=config_key,
            descriptor=descriptor,
            enabled=enabled,
            control_surface_id=control_surface_id,
            control_surface_path=control_surface_path,
            protocols=protocols,
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
