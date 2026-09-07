from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.display_rotation import DisplayRotation
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.display_descriptor import DisplayDescriptor


T = TypeVar("T", bound="DisplaySummaryListItem")


@_attrs_define
class DisplaySummaryListItem:
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
        rotation (DisplayRotation | Unset): Quarter turns applied to everything drawn on a display. A mounting
            fact about the panel, stored with the device's user settings so it
            holds across scenes, faces, and media layers alike.
    """

    circular: bool
    descriptor: DisplayDescriptor
    family: str
    height: int
    id: str
    name: str
    vendor: str
    width: int
    rotation: DisplayRotation | Unset = UNSET
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

        rotation: str | Unset = UNSET
        if not isinstance(self.rotation, Unset):
            rotation = self.rotation.value

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
        if rotation is not UNSET:
            field_dict["rotation"] = rotation

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

        _rotation = d.pop("rotation", UNSET)
        rotation: DisplayRotation | Unset
        if isinstance(_rotation, Unset):
            rotation = UNSET
        else:
            rotation = DisplayRotation(_rotation)

        display_summary_list_item = cls(
            circular=circular,
            descriptor=descriptor,
            family=family,
            height=height,
            id=id,
            name=name,
            vendor=vendor,
            width=width,
            rotation=rotation,
        )

        display_summary_list_item.additional_properties = d
        return display_summary_list_item

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
