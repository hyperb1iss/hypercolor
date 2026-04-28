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
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.driver_presentation import DriverPresentation
    from ..models.driver_transport_kind_type_6 import DriverTransportKindType6


T = TypeVar("T", bound="DriverProtocolDescriptor")


@_attrs_define
class DriverProtocolDescriptor:
    """Protocol descriptor contributed by a driver module.

    Attributes:
        display_name (str): Human-readable protocol or device label.
        driver_id (str): Driver module that owns this protocol.
        family_id (str): Stable device family identifier.
        protocol_id (str): Stable protocol implementation identifier.
        route_backend_id (str): Output backend ID that should route devices using this protocol.
        transport (DriverTransportKindType0 | DriverTransportKindType1 | DriverTransportKindType2 |
            DriverTransportKindType3 | DriverTransportKindType4 | DriverTransportKindType5 | DriverTransportKindType6):
            Transport category used by this protocol.
        model_id (None | str | Unset): Optional model identifier exposed by a driver-specific catalog.
        presentation (DriverPresentation | None | Unset): Optional presentation override for devices using this protocol.
        product_id (None | int | Unset): USB product ID when the protocol maps to a concrete USB device.
        vendor_id (None | int | Unset): USB vendor ID when the protocol maps to a concrete USB device.
    """

    display_name: str
    driver_id: str
    family_id: str
    protocol_id: str
    route_backend_id: str
    transport: (
        DriverTransportKindType0
        | DriverTransportKindType1
        | DriverTransportKindType2
        | DriverTransportKindType3
        | DriverTransportKindType4
        | DriverTransportKindType5
        | DriverTransportKindType6
    )
    model_id: None | str | Unset = UNSET
    presentation: DriverPresentation | None | Unset = UNSET
    product_id: None | int | Unset = UNSET
    vendor_id: None | int | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        display_name = self.display_name
        driver_id = self.driver_id
        family_id = self.family_id
        protocol_id = self.protocol_id
        route_backend_id = self.route_backend_id

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
        else:
            transport = self.transport.to_dict()

        model_id: None | str | Unset
        if isinstance(self.model_id, Unset):
            model_id = UNSET
        else:
            model_id = self.model_id

        presentation: dict[str, Any] | None | Unset
        if isinstance(self.presentation, Unset):
            presentation = UNSET
        elif self.presentation is None:
            presentation = None
        else:
            presentation = self.presentation.to_dict()

        product_id: None | int | Unset
        if isinstance(self.product_id, Unset):
            product_id = UNSET
        else:
            product_id = self.product_id

        vendor_id: None | int | Unset
        if isinstance(self.vendor_id, Unset):
            vendor_id = UNSET
        else:
            vendor_id = self.vendor_id

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "display_name": display_name,
                "driver_id": driver_id,
                "family_id": family_id,
                "protocol_id": protocol_id,
                "route_backend_id": route_backend_id,
                "transport": transport,
            }
        )
        if model_id is not UNSET:
            field_dict["model_id"] = model_id
        if presentation is not UNSET:
            field_dict["presentation"] = presentation
        if product_id is not UNSET:
            field_dict["product_id"] = product_id
        if vendor_id is not UNSET:
            field_dict["vendor_id"] = vendor_id

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.driver_presentation import DriverPresentation
        from ..models.driver_transport_kind_type_6 import DriverTransportKindType6

        d = dict(src_dict)
        display_name = d.pop("display_name")
        driver_id = d.pop("driver_id")
        family_id = d.pop("family_id")
        protocol_id = d.pop("protocol_id")
        route_backend_id = d.pop("route_backend_id")

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
        ):
            for transport_kind in (
                DriverTransportKindType0,
                DriverTransportKindType1,
                DriverTransportKindType2,
                DriverTransportKindType3,
                DriverTransportKindType4,
                DriverTransportKindType5,
            ):
                try:
                    if not isinstance(data, str):
                        raise TypeError()
                    return transport_kind(data)
                except (TypeError, ValueError, AttributeError, KeyError):
                    pass
            if not isinstance(data, dict):
                raise TypeError()
            return DriverTransportKindType6.from_dict(data)

        transport = _parse_transport(d.pop("transport"))

        def _parse_model_id(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        model_id = _parse_model_id(d.pop("model_id", UNSET))

        def _parse_presentation(data: object) -> DriverPresentation | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            if not isinstance(data, dict):
                raise TypeError()
            return DriverPresentation.from_dict(data)

        presentation = _parse_presentation(d.pop("presentation", UNSET))

        def _parse_product_id(data: object) -> None | int | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | int | Unset, data)

        product_id = _parse_product_id(d.pop("product_id", UNSET))

        def _parse_vendor_id(data: object) -> None | int | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | int | Unset, data)

        vendor_id = _parse_vendor_id(d.pop("vendor_id", UNSET))

        driver_protocol_descriptor = cls(
            display_name=display_name,
            driver_id=driver_id,
            family_id=family_id,
            protocol_id=protocol_id,
            route_backend_id=route_backend_id,
            transport=transport,
            model_id=model_id,
            presentation=presentation,
            product_id=product_id,
            vendor_id=vendor_id,
        )

        driver_protocol_descriptor.additional_properties = d
        return driver_protocol_descriptor

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
