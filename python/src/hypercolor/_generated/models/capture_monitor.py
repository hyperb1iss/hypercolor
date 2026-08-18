from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

T = TypeVar("T", bound="CaptureMonitor")


@_attrs_define
class CaptureMonitor:
    """
    Attributes:
        height (int):
        id (str):
        index (int):
        name (str):
        primary (bool):
        value (str):
        width (int):
    """

    height: int
    id: str
    index: int
    name: str
    primary: bool
    value: str
    width: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        height = self.height

        id = self.id

        index = self.index

        name = self.name

        primary = self.primary

        value = self.value

        width = self.width

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "height": height,
                "id": id,
                "index": index,
                "name": name,
                "primary": primary,
                "value": value,
                "width": width,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        height = d.pop("height")

        id = d.pop("id")

        index = d.pop("index")

        name = d.pop("name")

        primary = d.pop("primary")

        value = d.pop("value")

        width = d.pop("width")

        capture_monitor = cls(
            height=height,
            id=id,
            index=index,
            name=name,
            primary=primary,
            value=value,
            width=width,
        )

        capture_monitor.additional_properties = d
        return capture_monitor

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
