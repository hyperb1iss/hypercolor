from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.control_group_kind import ControlGroupKind
from ..types import UNSET, Unset

T = TypeVar("T", bound="ControlGroupDescriptor")


@_attrs_define
class ControlGroupDescriptor:
    """Semantic group descriptor.

    Attributes:
        id (str):
        kind (ControlGroupKind): Semantic group kind.
        label (str): Human-readable label.
        ordering (int): Stable ordering hint.
        description (None | str | Unset): Optional help text.
    """

    id: str
    kind: ControlGroupKind
    label: str
    ordering: int
    description: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        id = self.id

        kind = self.kind.value

        label = self.label

        ordering = self.ordering

        description: None | str | Unset
        if isinstance(self.description, Unset):
            description = UNSET
        else:
            description = self.description

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "id": id,
                "kind": kind,
                "label": label,
                "ordering": ordering,
            }
        )
        if description is not UNSET:
            field_dict["description"] = description

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        id = d.pop("id")

        kind = ControlGroupKind(d.pop("kind"))

        label = d.pop("label")

        ordering = d.pop("ordering")

        def _parse_description(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        description = _parse_description(d.pop("description", UNSET))

        control_group_descriptor = cls(
            id=id,
            kind=kind,
            label=label,
            ordering=ordering,
            description=description,
        )

        control_group_descriptor.additional_properties = d
        return control_group_descriptor

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
