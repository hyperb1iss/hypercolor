from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="SceneSummary")


@_attrs_define
class SceneSummary:
    """One saved scene as listed by `GET /api/v1/scenes`.

    Attributes:
        id (str):
        name (str):
        description (None | str | Unset):
        enabled (bool | Unset): Whether the scene participates in activation. Defaults true for
            daemons that predate the field.
        mutation_mode (str | Unset): Live vs snapshot-locked. Lets scene pickers mark locked scenes
            without inferring lock state from the live scene kind.
        priority (int | Unset):
    """

    id: str
    name: str
    description: None | str | Unset = UNSET
    enabled: bool | Unset = UNSET
    mutation_mode: str | Unset = UNSET
    priority: int | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        id = self.id

        name = self.name

        description: None | str | Unset
        if isinstance(self.description, Unset):
            description = UNSET
        else:
            description = self.description

        enabled = self.enabled

        mutation_mode = self.mutation_mode

        priority = self.priority

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "id": id,
                "name": name,
            }
        )
        if description is not UNSET:
            field_dict["description"] = description
        if enabled is not UNSET:
            field_dict["enabled"] = enabled
        if mutation_mode is not UNSET:
            field_dict["mutation_mode"] = mutation_mode
        if priority is not UNSET:
            field_dict["priority"] = priority

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        id = d.pop("id")

        name = d.pop("name")

        def _parse_description(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        description = _parse_description(d.pop("description", UNSET))

        enabled = d.pop("enabled", UNSET)

        mutation_mode = d.pop("mutation_mode", UNSET)

        priority = d.pop("priority", UNSET)

        scene_summary = cls(
            id=id,
            name=name,
            description=description,
            enabled=enabled,
            mutation_mode=mutation_mode,
            priority=priority,
        )

        scene_summary.additional_properties = d
        return scene_summary

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
