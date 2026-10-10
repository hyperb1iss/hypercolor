from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

if TYPE_CHECKING:
    from ..models.software_conflict import SoftwareConflict


T = TypeVar("T", bound="SoftwareConflictsStatus")


@_attrs_define
class SoftwareConflictsStatus:
    """What the latest conflict scan found.

    Attributes:
        conflicts (list[SoftwareConflict]): Competing software that is running now, in catalog order.
        scanned (bool): Whether a scan has finished since the daemon started.
        supported (bool): Whether this host can list running software. When false,
            `conflicts` is always empty and proves nothing.
    """

    conflicts: list[SoftwareConflict]
    scanned: bool
    supported: bool
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        conflicts = []
        for conflicts_item_data in self.conflicts:
            conflicts_item = conflicts_item_data.to_dict()
            conflicts.append(conflicts_item)

        scanned = self.scanned

        supported = self.supported

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "conflicts": conflicts,
                "scanned": scanned,
                "supported": supported,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.software_conflict import SoftwareConflict

        d = dict(src_dict)
        conflicts = []
        _conflicts = d.pop("conflicts")
        for conflicts_item_data in _conflicts:
            conflicts_item = SoftwareConflict.from_dict(conflicts_item_data)

            conflicts.append(conflicts_item)

        scanned = d.pop("scanned")

        supported = d.pop("supported")

        software_conflicts_status = cls(
            conflicts=conflicts,
            scanned=scanned,
            supported=supported,
        )

        software_conflicts_status.additional_properties = d
        return software_conflicts_status

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
