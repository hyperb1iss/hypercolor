from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="ZoneMember")


@_attrs_define
class ZoneMember:
    """One zone membership: a device segment's assignment, with its own
    identity (Spec 78 §1.2).

        Attributes:
            device_id (str): Backend device identifier (`"<backend>:<device_id>"`).
            id (str): A zone membership's identity — wire-transparent, unique within its
                zone, which is all its zone-scoped route needs.
            name (str): Human-readable name carried from the layout output.
            segment (None | str | Unset): The device segment this membership assigns. `None` for
                single-segment devices.
    """

    device_id: str
    id: str
    name: str
    segment: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        device_id = self.device_id

        id = self.id

        name = self.name

        segment: None | str | Unset
        if isinstance(self.segment, Unset):
            segment = UNSET
        else:
            segment = self.segment

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "device_id": device_id,
                "id": id,
                "name": name,
            }
        )
        if segment is not UNSET:
            field_dict["segment"] = segment

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        device_id = d.pop("device_id")

        id = d.pop("id")

        name = d.pop("name")

        def _parse_segment(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        segment = _parse_segment(d.pop("segment", UNSET))

        zone_member = cls(
            device_id=device_id,
            id=id,
            name=name,
            segment=segment,
        )

        zone_member.additional_properties = d
        return zone_member

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
