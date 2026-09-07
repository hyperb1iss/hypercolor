from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define

from ..types import UNSET, Unset

T = TypeVar("T", bound="AssignMembersRequest")


@_attrs_define
class AssignMembersRequest:
    """`POST /scene/zones/{zone}/members` — assign device segments.

    The request names a device and its segments; the response's zone
    resource carries the minted membership ids (Spec 78 §1.2).

        Attributes:
            device_id (str):
            segments (list[str] | Unset): Segment names to assign; empty assigns the whole device on
                single-segment hardware.
    """

    device_id: str
    segments: list[str] | Unset = UNSET

    def to_dict(self) -> dict[str, Any]:
        device_id = self.device_id

        segments: list[str] | Unset = UNSET
        if not isinstance(self.segments, Unset):
            segments = self.segments

        field_dict: dict[str, Any] = {}

        field_dict.update(
            {
                "device_id": device_id,
            }
        )
        if segments is not UNSET:
            field_dict["segments"] = segments

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        device_id = d.pop("device_id")

        segments = cast(list[str], d.pop("segments", UNSET))

        assign_members_request = cls(
            device_id=device_id,
            segments=segments,
        )

        return assign_members_request
