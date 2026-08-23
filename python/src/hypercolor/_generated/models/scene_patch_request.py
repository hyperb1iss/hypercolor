from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define

from ..types import UNSET, Unset

T = TypeVar("T", bound="ScenePatchRequest")


@_attrs_define
class ScenePatchRequest:
    """`PATCH /scene` — scene-level fields only (Spec 78 §1.2).

    Attributes:
        name (None | str | Unset): Rename; rejected for the default scene.
        unassigned_behavior (None | str | Unset):
    """

    name: None | str | Unset = UNSET
    unassigned_behavior: None | str | Unset = UNSET

    def to_dict(self) -> dict[str, Any]:
        name: None | str | Unset
        if isinstance(self.name, Unset):
            name = UNSET
        else:
            name = self.name

        unassigned_behavior: None | str | Unset
        if isinstance(self.unassigned_behavior, Unset):
            unassigned_behavior = UNSET
        else:
            unassigned_behavior = self.unassigned_behavior

        field_dict: dict[str, Any] = {}

        field_dict.update({})
        if name is not UNSET:
            field_dict["name"] = name
        if unassigned_behavior is not UNSET:
            field_dict["unassigned_behavior"] = unassigned_behavior

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)

        def _parse_name(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        name = _parse_name(d.pop("name", UNSET))

        def _parse_unassigned_behavior(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        unassigned_behavior = _parse_unassigned_behavior(
            d.pop("unassigned_behavior", UNSET)
        )

        scene_patch_request = cls(
            name=name,
            unassigned_behavior=unassigned_behavior,
        )

        return scene_patch_request
