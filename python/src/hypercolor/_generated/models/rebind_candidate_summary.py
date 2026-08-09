from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="RebindCandidateSummary")


@_attrs_define
class RebindCandidateSummary:
    """One attached device offered as a re-bind target.

    Attributes:
        device_id (str):
        layout_device_id (str): The layout binding id this device currently derives.
        name (str):
        status (str):
        portable_key (None | str | Unset): The device's portable key. Only claimed devices can inherit a
            binding durably; a claimless candidate re-binds by layout edit.
    """

    device_id: str
    layout_device_id: str
    name: str
    status: str
    portable_key: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        device_id = self.device_id

        layout_device_id = self.layout_device_id

        name = self.name

        status = self.status

        portable_key: None | str | Unset
        if isinstance(self.portable_key, Unset):
            portable_key = UNSET
        else:
            portable_key = self.portable_key

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "device_id": device_id,
                "layout_device_id": layout_device_id,
                "name": name,
                "status": status,
            }
        )
        if portable_key is not UNSET:
            field_dict["portable_key"] = portable_key

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        device_id = d.pop("device_id")

        layout_device_id = d.pop("layout_device_id")

        name = d.pop("name")

        status = d.pop("status")

        def _parse_portable_key(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        portable_key = _parse_portable_key(d.pop("portable_key", UNSET))

        rebind_candidate_summary = cls(
            device_id=device_id,
            layout_device_id=layout_device_id,
            name=name,
            status=status,
            portable_key=portable_key,
        )

        rebind_candidate_summary.additional_properties = d
        return rebind_candidate_summary

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
