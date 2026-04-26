from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.driver_module_kind import DriverModuleKind
from ..models.driver_transport_kind_type_0 import DriverTransportKindType0
from ..models.driver_transport_kind_type_1 import DriverTransportKindType1
from ..models.driver_transport_kind_type_2 import DriverTransportKindType2
from ..models.driver_transport_kind_type_3 import DriverTransportKindType3
from ..models.driver_transport_kind_type_4 import DriverTransportKindType4
from ..models.driver_transport_kind_type_5 import DriverTransportKindType5
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.driver_capability_set import DriverCapabilitySet
    from ..models.driver_transport_kind_type_6 import DriverTransportKindType6


T = TypeVar("T", bound="DriverModuleDescriptor")


@_attrs_define
class DriverModuleDescriptor:
    """Stable module descriptor for native and future Wasm driver registries.

    Attributes:
        api_schema_version (int): Version of the driver-facing API schema.
        capabilities (DriverCapabilitySet): Capability flags exposed by a driver module.
        config_version (int): Version of this driver's config schema.
        default_enabled (bool): Whether this driver should be enabled by default.
        display_name (str): Human-readable driver name.
        id (str): Stable driver identifier.
        module_kind (DriverModuleKind): High-level module category used for driver registry introspection.
        transports (list[DriverTransportKindType0 | DriverTransportKindType1 | DriverTransportKindType2 |
            DriverTransportKindType3 | DriverTransportKindType4 | DriverTransportKindType5 | DriverTransportKindType6]):
            Transport categories used by this driver.
        vendor_name (None | str | Unset): Optional vendor or organization name.
    """

    api_schema_version: int
    capabilities: DriverCapabilitySet
    config_version: int
    default_enabled: bool
    display_name: str
    id: str
    module_kind: DriverModuleKind
    transports: list[
        DriverTransportKindType0
        | DriverTransportKindType1
        | DriverTransportKindType2
        | DriverTransportKindType3
        | DriverTransportKindType4
        | DriverTransportKindType5
        | DriverTransportKindType6
    ]
    vendor_name: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        api_schema_version = self.api_schema_version

        capabilities = self.capabilities.to_dict()

        config_version = self.config_version

        default_enabled = self.default_enabled

        display_name = self.display_name

        id = self.id

        module_kind = self.module_kind.value

        transports = []
        for transports_item_data in self.transports:
            transports_item: dict[str, Any] | str
            if isinstance(transports_item_data, DriverTransportKindType0):
                transports_item = transports_item_data.value
            elif isinstance(transports_item_data, DriverTransportKindType1):
                transports_item = transports_item_data.value
            elif isinstance(transports_item_data, DriverTransportKindType2):
                transports_item = transports_item_data.value
            elif isinstance(transports_item_data, DriverTransportKindType3):
                transports_item = transports_item_data.value
            elif isinstance(transports_item_data, DriverTransportKindType4):
                transports_item = transports_item_data.value
            elif isinstance(transports_item_data, DriverTransportKindType5):
                transports_item = transports_item_data.value
            else:
                transports_item = transports_item_data.to_dict()

            transports.append(transports_item)

        vendor_name: None | str | Unset
        if isinstance(self.vendor_name, Unset):
            vendor_name = UNSET
        else:
            vendor_name = self.vendor_name

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "api_schema_version": api_schema_version,
                "capabilities": capabilities,
                "config_version": config_version,
                "default_enabled": default_enabled,
                "display_name": display_name,
                "id": id,
                "module_kind": module_kind,
                "transports": transports,
            }
        )
        if vendor_name is not UNSET:
            field_dict["vendor_name"] = vendor_name

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.driver_capability_set import DriverCapabilitySet
        from ..models.driver_transport_kind_type_6 import DriverTransportKindType6

        d = dict(src_dict)
        api_schema_version = d.pop("api_schema_version")

        capabilities = DriverCapabilitySet.from_dict(d.pop("capabilities"))

        config_version = d.pop("config_version")

        default_enabled = d.pop("default_enabled")

        display_name = d.pop("display_name")

        id = d.pop("id")

        module_kind = DriverModuleKind(d.pop("module_kind"))

        transports = []
        _transports = d.pop("transports")
        for transports_item_data in _transports:

            def _parse_transports_item(
                data: object,
            ) -> (
                DriverTransportKindType0
                | DriverTransportKindType1
                | DriverTransportKindType2
                | DriverTransportKindType3
                | DriverTransportKindType4
                | DriverTransportKindType5
                | DriverTransportKindType6
            ):
                try:
                    if not isinstance(data, str):
                        raise TypeError()
                    componentsschemas_driver_transport_kind_type_0 = (
                        DriverTransportKindType0(data)
                    )

                    return componentsschemas_driver_transport_kind_type_0
                except (TypeError, ValueError, AttributeError, KeyError):
                    pass
                try:
                    if not isinstance(data, str):
                        raise TypeError()
                    componentsschemas_driver_transport_kind_type_1 = (
                        DriverTransportKindType1(data)
                    )

                    return componentsschemas_driver_transport_kind_type_1
                except (TypeError, ValueError, AttributeError, KeyError):
                    pass
                try:
                    if not isinstance(data, str):
                        raise TypeError()
                    componentsschemas_driver_transport_kind_type_2 = (
                        DriverTransportKindType2(data)
                    )

                    return componentsschemas_driver_transport_kind_type_2
                except (TypeError, ValueError, AttributeError, KeyError):
                    pass
                try:
                    if not isinstance(data, str):
                        raise TypeError()
                    componentsschemas_driver_transport_kind_type_3 = (
                        DriverTransportKindType3(data)
                    )

                    return componentsschemas_driver_transport_kind_type_3
                except (TypeError, ValueError, AttributeError, KeyError):
                    pass
                try:
                    if not isinstance(data, str):
                        raise TypeError()
                    componentsschemas_driver_transport_kind_type_4 = (
                        DriverTransportKindType4(data)
                    )

                    return componentsschemas_driver_transport_kind_type_4
                except (TypeError, ValueError, AttributeError, KeyError):
                    pass
                try:
                    if not isinstance(data, str):
                        raise TypeError()
                    componentsschemas_driver_transport_kind_type_5 = (
                        DriverTransportKindType5(data)
                    )

                    return componentsschemas_driver_transport_kind_type_5
                except (TypeError, ValueError, AttributeError, KeyError):
                    pass
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_driver_transport_kind_type_6 = (
                    DriverTransportKindType6.from_dict(data)
                )

                return componentsschemas_driver_transport_kind_type_6

            transports_item = _parse_transports_item(transports_item_data)

            transports.append(transports_item)

        def _parse_vendor_name(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        vendor_name = _parse_vendor_name(d.pop("vendor_name", UNSET))

        driver_module_descriptor = cls(
            api_schema_version=api_schema_version,
            capabilities=capabilities,
            config_version=config_version,
            default_enabled=default_enabled,
            display_name=display_name,
            id=id,
            module_kind=module_kind,
            transports=transports,
            vendor_name=vendor_name,
        )

        driver_module_descriptor.additional_properties = d
        return driver_module_descriptor

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
