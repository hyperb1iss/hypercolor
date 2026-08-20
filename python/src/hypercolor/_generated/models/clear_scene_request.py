from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define

from ..types import UNSET, Unset

T = TypeVar("T", bound="ClearSceneRequest")


@_attrs_define
class ClearSceneRequest:
    """`POST /scene/clear` — the "stop" gesture (Spec 78 §1.2).

    Attributes:
        zone (None | str | Unset): Clear one non-display zone's stack; omitted clears every non-display zone.
    """

    zone: None | str | Unset = UNSET

    def to_dict(self) -> dict[str, Any]:
        zone: None | str | Unset
        if isinstance(self.zone, Unset):
            zone = UNSET
        else:
            zone = self.zone

        field_dict: dict[str, Any] = {}

        field_dict.update({})
        if zone is not UNSET:
            field_dict["zone"] = zone

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)

        def _parse_zone(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        zone = _parse_zone(d.pop("zone", UNSET))

        clear_scene_request = cls(
            zone=zone,
        )

        return clear_scene_request
