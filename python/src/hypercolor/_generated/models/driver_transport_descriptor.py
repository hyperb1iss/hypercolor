from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.driver_transport_kind_type_0 import DriverTransportKindType0
from ..models.driver_transport_kind_type_1 import DriverTransportKindType1
from ..models.driver_transport_kind_type_2 import DriverTransportKindType2
from ..models.driver_transport_kind_type_3 import DriverTransportKindType3
from ..models.driver_transport_kind_type_4 import DriverTransportKindType4
from ..models.driver_transport_kind_type_5 import DriverTransportKindType5
from ..models.driver_transport_kind_type_6 import DriverTransportKindType6

if TYPE_CHECKING:
    from ..models.driver_transport_availability_type_0 import (
        DriverTransportAvailabilityType0,
    )
    from ..models.driver_transport_availability_type_1 import (
        DriverTransportAvailabilityType1,
    )
    from ..models.driver_transport_kind_type_7 import DriverTransportKindType7


T = TypeVar("T", bound="DriverTransportDescriptor")


@_attrs_define
class DriverTransportDescriptor:
    """One transport category advertised by a driver module.

    Attributes:
        availability (DriverTransportAvailabilityType0 | DriverTransportAvailabilityType1): Whether a driver transport
            can run on the current platform.
        kind (DriverTransportKindType0 | DriverTransportKindType1 | DriverTransportKindType2 | DriverTransportKindType3
            | DriverTransportKindType4 | DriverTransportKindType5 | DriverTransportKindType6 | DriverTransportKindType7):
            API-facing transport category for a driver module.
    """

    availability: DriverTransportAvailabilityType0 | DriverTransportAvailabilityType1
    kind: (
        DriverTransportKindType0
        | DriverTransportKindType1
        | DriverTransportKindType2
        | DriverTransportKindType3
        | DriverTransportKindType4
        | DriverTransportKindType5
        | DriverTransportKindType6
        | DriverTransportKindType7
    )
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
        elif isinstance(self.kind, DriverTransportKindType1):
            kind = self.kind.value
        elif isinstance(self.kind, DriverTransportKindType2):
            kind = self.kind.value
        elif isinstance(self.kind, DriverTransportKindType3):
            kind = self.kind.value
        elif isinstance(self.kind, DriverTransportKindType4):
            kind = self.kind.value
        elif isinstance(self.kind, DriverTransportKindType5):
            kind = self.kind.value
        elif isinstance(self.kind, DriverTransportKindType6):
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
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.driver_transport_availability_type_0 import (
            DriverTransportAvailabilityType0,
        )
        from ..models.driver_transport_availability_type_1 import (
            DriverTransportAvailabilityType1,
        )
        from ..models.driver_transport_kind_type_7 import DriverTransportKindType7

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
