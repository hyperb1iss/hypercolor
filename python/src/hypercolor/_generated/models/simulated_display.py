from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar
from uuid import UUID

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="SimulatedDisplay")


@_attrs_define
class SimulatedDisplay:
    """One simulated display as `/api/v1/simulators/displays` renders it.

    This is both the stored configuration and the resource every route
    in the family returns.

        Attributes:
            height (int):
            id (UUID): Opaque, globally unique device identifier.

                Wraps a `UUIDv7` so identifiers are time-ordered and safe to use as
                database keys, map keys, and log correlation IDs.
            name (str):
            width (int):
            circular (bool | Unset):
            enabled (bool | Unset):
    """

    height: int
    id: UUID
    name: str
    width: int
    circular: bool | Unset = UNSET
    enabled: bool | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        height = self.height

        id = str(self.id)

        name = self.name

        width = self.width

        circular = self.circular

        enabled = self.enabled

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "height": height,
                "id": id,
                "name": name,
                "width": width,
            }
        )
        if circular is not UNSET:
            field_dict["circular"] = circular
        if enabled is not UNSET:
            field_dict["enabled"] = enabled

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        height = d.pop("height")

        id = UUID(d.pop("id"))

        name = d.pop("name")

        width = d.pop("width")

        circular = d.pop("circular", UNSET)

        enabled = d.pop("enabled", UNSET)

        simulated_display = cls(
            height=height,
            id=id,
            name=name,
            width=width,
            circular=circular,
            enabled=enabled,
        )

        simulated_display.additional_properties = d
        return simulated_display

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
