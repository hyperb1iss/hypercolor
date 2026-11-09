from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..models.driver_transport_kind_type_0 import DriverTransportKindType0

if TYPE_CHECKING:
    from ..models.driver_transport_availability_type_0 import (
        DriverTransportAvailabilityType0,
    )
    from ..models.driver_transport_availability_type_1 import (
        DriverTransportAvailabilityType1,
    )
    from ..models.driver_transport_kind_type_1 import DriverTransportKindType1


T = TypeVar("T", bound="DriverTransportDescriptor")


@_attrs_define
class DriverTransportDescriptor:
    """One transport category advertised by a driver module.

    Attributes:
        availability (DriverTransportAvailabilityType0 | DriverTransportAvailabilityType1): Whether a driver transport
            can run on the current platform.
        kind (DriverTransportKindType0 | DriverTransportKindType1): API-facing transport category for a driver module.
    """

    availability: DriverTransportAvailabilityType0 | DriverTransportAvailabilityType1
    kind: DriverTransportKindType0 | DriverTransportKindType1
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.driver_transport_availability_type_0 import (
            DriverTransportAvailabilityType0,
        )

        availability: dict[str, Any]
        if isinstance(self.availability, DriverTransportAvailabilityType0):
            availability = self.availability.to_dict()
        else:
            availability = self.availability.to_dict()

        kind: dict[str, Any] | str
        if isinstance(self.kind, DriverTransportKindType0):
            kind = self.kind.value
        else:
            kind = self.kind.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "availability": availability,
                "kind": kind,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.driver_transport_availability_type_0 import (
            DriverTransportAvailabilityType0,
        )
        from ..models.driver_transport_availability_type_1 import (
            DriverTransportAvailabilityType1,
        )
        from ..models.driver_transport_kind_type_1 import (
            DriverTransportKindType1,
        )

        d = dict(src_dict)

        def _parse_availability(
            data: object,
        ) -> DriverTransportAvailabilityType0 | DriverTransportAvailabilityType1:
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_driver_transport_availability_type_0 = (
                    DriverTransportAvailabilityType0.from_dict(data)
                )

                return componentsschemas_driver_transport_availability_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            if not isinstance(data, dict):
                raise TypeError()
            componentsschemas_driver_transport_availability_type_1 = (
                DriverTransportAvailabilityType1.from_dict(data)
            )

            return componentsschemas_driver_transport_availability_type_1

        availability = _parse_availability(d.pop("availability"))

        def _parse_kind(
            data: object,
        ) -> DriverTransportKindType0 | DriverTransportKindType1:
            try:
                if not isinstance(data, str):
                    raise TypeError()
                componentsschemas_driver_transport_kind_type_0 = (
                    DriverTransportKindType0(data)
                )

                return componentsschemas_driver_transport_kind_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            if not isinstance(data, dict):
                raise TypeError()
            componentsschemas_driver_transport_kind_type_1 = (
                DriverTransportKindType1.from_dict(data)
            )

            return componentsschemas_driver_transport_kind_type_1

        kind = _parse_kind(d.pop("kind"))

        driver_transport_descriptor = cls(
            availability=availability,
            kind=kind,
        )

        driver_transport_descriptor.additional_properties = d
        return driver_transport_descriptor

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
