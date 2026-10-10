from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

T = TypeVar("T", bound="SoftwareConflict")


@_attrs_define
class SoftwareConflict:
    """RGB software running on the host that competes with Hypercolor for
    devices.

        Attributes:
            all_drivers (bool): Whether it competes with every driver, as whole-system RGB suites do.
            driver_ids (list[str]): Hypercolor driver ids whose devices this software holds or fights
                over. Empty when [`Self::all_drivers`] is set.
            id (str): Stable catalog id (`signalrgb`, `lian_li_l_connect`).
            matched (list[str]): Process and service names that matched, as the host reported them.
            name (str): Product name for display.
            remedy (str): What the user should do about it.
            smbus (bool): Whether it drives SMBus lighting (motherboard, RAM, GPU).
    """

    all_drivers: bool
    driver_ids: list[str]
    id: str
    matched: list[str]
    name: str
    remedy: str
    smbus: bool
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        all_drivers = self.all_drivers

        driver_ids = self.driver_ids

        id = self.id

        matched = self.matched

        name = self.name

        remedy = self.remedy

        smbus = self.smbus

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "all_drivers": all_drivers,
                "driver_ids": driver_ids,
                "id": id,
                "matched": matched,
                "name": name,
                "remedy": remedy,
                "smbus": smbus,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        all_drivers = d.pop("all_drivers")

        driver_ids = cast(list[str], d.pop("driver_ids"))

        id = d.pop("id")

        matched = cast(list[str], d.pop("matched"))

        name = d.pop("name")

        remedy = d.pop("remedy")

        smbus = d.pop("smbus")

        software_conflict = cls(
            all_drivers=all_drivers,
            driver_ids=driver_ids,
            id=id,
            matched=matched,
            name=name,
            remedy=remedy,
            smbus=smbus,
        )

        software_conflict.additional_properties = d
        return software_conflict

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
