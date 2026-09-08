from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="UnclaimedDevice")


@_attrs_define
class UnclaimedDevice:
    """A USB device the host can see that no enabled native driver claims.

    `claimable_by` names the native driver whose protocol database matches
    the device when that driver is disabled by config; `None` means no
    native protocol exists for the vendor/product pair at all.

        Attributes:
            product_id (int):
            vendor_id (int):
            bus_path (None | str | Unset): Host bus path (`<bus>-<port chain>`), when the platform reports one.
            claimable_by (None | str | Unset):
            interface_classes (list[int] | Unset): USB interface class codes of the active configuration, sorted and
                deduplicated; empty where the platform does not expose them.
            manufacturer (None | str | Unset):
            product (None | str | Unset):
            serial (None | str | Unset):
    """

    product_id: int
    vendor_id: int
    bus_path: None | str | Unset = UNSET
    claimable_by: None | str | Unset = UNSET
    interface_classes: list[int] | Unset = UNSET
    manufacturer: None | str | Unset = UNSET
    product: None | str | Unset = UNSET
    serial: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        product_id = self.product_id

        vendor_id = self.vendor_id

        bus_path: None | str | Unset
        if isinstance(self.bus_path, Unset):
            bus_path = UNSET
        else:
            bus_path = self.bus_path

        claimable_by: None | str | Unset
        if isinstance(self.claimable_by, Unset):
            claimable_by = UNSET
        else:
            claimable_by = self.claimable_by

        interface_classes: list[int] | Unset = UNSET
        if not isinstance(self.interface_classes, Unset):
            interface_classes = self.interface_classes

        manufacturer: None | str | Unset
        if isinstance(self.manufacturer, Unset):
            manufacturer = UNSET
        else:
            manufacturer = self.manufacturer

        product: None | str | Unset
        if isinstance(self.product, Unset):
            product = UNSET
        else:
            product = self.product

        serial: None | str | Unset
        if isinstance(self.serial, Unset):
            serial = UNSET
        else:
            serial = self.serial

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "product_id": product_id,
                "vendor_id": vendor_id,
            }
        )
        if bus_path is not UNSET:
            field_dict["bus_path"] = bus_path
        if claimable_by is not UNSET:
            field_dict["claimable_by"] = claimable_by
        if interface_classes is not UNSET:
            field_dict["interface_classes"] = interface_classes
        if manufacturer is not UNSET:
            field_dict["manufacturer"] = manufacturer
        if product is not UNSET:
            field_dict["product"] = product
        if serial is not UNSET:
            field_dict["serial"] = serial

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        product_id = d.pop("product_id")

        vendor_id = d.pop("vendor_id")

        def _parse_bus_path(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        bus_path = _parse_bus_path(d.pop("bus_path", UNSET))

        def _parse_claimable_by(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        claimable_by = _parse_claimable_by(d.pop("claimable_by", UNSET))

        interface_classes = cast(list[int], d.pop("interface_classes", UNSET))

        def _parse_manufacturer(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        manufacturer = _parse_manufacturer(d.pop("manufacturer", UNSET))

        def _parse_product(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        product = _parse_product(d.pop("product", UNSET))

        def _parse_serial(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        serial = _parse_serial(d.pop("serial", UNSET))

        unclaimed_device = cls(
            product_id=product_id,
            vendor_id=vendor_id,
            bus_path=bus_path,
            claimable_by=claimable_by,
            interface_classes=interface_classes,
            manufacturer=manufacturer,
            product=product,
            serial=serial,
        )

        unclaimed_device.additional_properties = d
        return unclaimed_device

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
