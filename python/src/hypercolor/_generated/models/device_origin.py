from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.driver_transport_kind_type_0 import DriverTransportKindType0
from ..models.driver_transport_kind_type_1 import DriverTransportKindType1
from ..models.driver_transport_kind_type_2 import DriverTransportKindType2
from ..models.driver_transport_kind_type_3 import DriverTransportKindType3
from ..models.driver_transport_kind_type_4 import DriverTransportKindType4
from ..models.driver_transport_kind_type_5 import DriverTransportKindType5
from ..models.driver_transport_kind_type_6 import DriverTransportKindType6
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.driver_transport_kind_type_7 import DriverTransportKindType7


T = TypeVar("T", bound="DeviceOrigin")


@_attrs_define
class DeviceOrigin:
    """Origin metadata that separates device ownership from output routing.

    Attributes:
        backend_id (str): Output backend responsible for writing frames.
        driver_id (str): Driver module that owns discovery, semantics, and presentation.
        transport (DriverTransportKindType0 | DriverTransportKindType1 | DriverTransportKindType2 |
            DriverTransportKindType3 | DriverTransportKindType4 | DriverTransportKindType5 | DriverTransportKindType6 |
            DriverTransportKindType7): API-facing transport category for a driver module.
        protocol_id (None | str | Unset): Optional protocol implementation selected by the driver/backend.
    """

    backend_id: str
    driver_id: str
    transport: (
        DriverTransportKindType0
        | DriverTransportKindType1
        | DriverTransportKindType2
        | DriverTransportKindType3
        | DriverTransportKindType4
        | DriverTransportKindType5
        | DriverTransportKindType6
        | DriverTransportKindType7
    )
    protocol_id: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        backend_id = self.backend_id

        driver_id = self.driver_id

        transport: dict[str, Any] | str
        if isinstance(self.transport, DriverTransportKindType0):
            transport = self.transport.value
        elif isinstance(self.transport, DriverTransportKindType1):
            transport = self.transport.value
        elif isinstance(self.transport, DriverTransportKindType2):
            transport = self.transport.value
        elif isinstance(self.transport, DriverTransportKindType3):
            transport = self.transport.value
        elif isinstance(self.transport, DriverTransportKindType4):
            transport = self.transport.value
        elif isinstance(self.transport, DriverTransportKindType5):
            transport = self.transport.value
        elif isinstance(self.transport, DriverTransportKindType6):
            transport = self.transport.value
        else:
            transport = self.transport.to_dict()

        protocol_id: None | str | Unset
        if isinstance(self.protocol_id, Unset):
            protocol_id = UNSET
        else:
            protocol_id = self.protocol_id

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "backend_id": backend_id,
                "driver_id": driver_id,
                "transport": transport,
            }
        )
        if protocol_id is not UNSET:
            field_dict["protocol_id"] = protocol_id

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.driver_transport_kind_type_7 import DriverTransportKindType7

        d = dict(src_dict)
        backend_id = d.pop("backend_id")

        driver_id = d.pop("driver_id")

        def _parse_transport(
            data: object,
        ) -> (
            DriverTransportKindType0
            | DriverTransportKindType1
            | DriverTransportKindType2
            | DriverTransportKindType3
            | DriverTransportKindType4
            | DriverTransportKindType5
            | DriverTransportKindType6
            | DriverTransportKindType7
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
            try:
                if not isinstance(data, str):
                    raise TypeError()
                componentsschemas_driver_transport_kind_type_6 = (
                    DriverTransportKindType6(data)
                )

                return componentsschemas_driver_transport_kind_type_6
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            if not isinstance(data, dict):
                raise TypeError()
            componentsschemas_driver_transport_kind_type_7 = (
                DriverTransportKindType7.from_dict(data)
            )

            return componentsschemas_driver_transport_kind_type_7

        transport = _parse_transport(d.pop("transport"))

        def _parse_protocol_id(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        protocol_id = _parse_protocol_id(d.pop("protocol_id", UNSET))

        device_origin = cls(
            backend_id=backend_id,
            driver_id=driver_id,
            transport=transport,
            protocol_id=protocol_id,
        )

        device_origin.additional_properties = d
        return device_origin

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
