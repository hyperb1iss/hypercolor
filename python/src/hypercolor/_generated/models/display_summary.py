from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.display_descriptor import DisplayDescriptor


T = TypeVar("T", bound="DisplaySummary")


@_attrs_define
class DisplaySummary:
    """Summary row from `GET /api/v1/displays`.

    Attributes:
        circular (bool):
        descriptor (DisplayDescriptor): Everything a face needs to know about the surface it renders on.
        family (str):
        height (int):
        id (str):
        name (str):
        vendor (str):
        width (int):
    """

    circular: bool
    descriptor: DisplayDescriptor
    family: str
    height: int
    id: str
    name: str
    vendor: str
    width: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        circular = self.circular

        descriptor = self.descriptor.to_dict()

        family = self.family

        height = self.height

        id = self.id

        name = self.name

        vendor = self.vendor

        width = self.width

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "circular": circular,
                "descriptor": descriptor,
                "family": family,
                "height": height,
                "id": id,
                "name": name,
                "vendor": vendor,
                "width": width,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.display_descriptor import DisplayDescriptor

        d = dict(src_dict)
        circular = d.pop("circular")

        descriptor = DisplayDescriptor.from_dict(d.pop("descriptor"))

        family = d.pop("family")

        height = d.pop("height")

        id = d.pop("id")

        name = d.pop("name")

        vendor = d.pop("vendor")

        width = d.pop("width")

        display_summary = cls(
            circular=circular,
            descriptor=descriptor,
            family=family,
            height=height,
            id=id,
            name=name,
            vendor=vendor,
            width=width,
        )

        display_summary.additional_properties = d
        return display_summary

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
