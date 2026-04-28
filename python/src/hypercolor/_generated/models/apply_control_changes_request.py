from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.control_change import ControlChange


T = TypeVar("T", bound="ApplyControlChangesRequest")


@_attrs_define
class ApplyControlChangesRequest:
    """Request to apply one or more control changes.

    Attributes:
        changes (list[ControlChange]): Changes to apply atomically.
        surface_id (str):
        dry_run (bool | Unset): Validate without mutating state.
        expected_revision (int | None | Unset):
    """

    changes: list[ControlChange]
    surface_id: str
    dry_run: bool | Unset = UNSET
    expected_revision: int | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        changes = []
        for changes_item_data in self.changes:
            changes_item = changes_item_data.to_dict()
            changes.append(changes_item)

        surface_id = self.surface_id

        dry_run = self.dry_run

        expected_revision: int | None | Unset
        if isinstance(self.expected_revision, Unset):
            expected_revision = UNSET
        else:
            expected_revision = self.expected_revision

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "changes": changes,
                "surface_id": surface_id,
            }
        )
        if dry_run is not UNSET:
            field_dict["dry_run"] = dry_run
        if expected_revision is not UNSET:
            field_dict["expected_revision"] = expected_revision

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.control_change import ControlChange

        d = dict(src_dict)
        changes = []
        _changes = d.pop("changes")
        for changes_item_data in _changes:
            changes_item = ControlChange.from_dict(changes_item_data)

            changes.append(changes_item)

        surface_id = d.pop("surface_id")

        dry_run = d.pop("dry_run", UNSET)

        def _parse_expected_revision(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        expected_revision = _parse_expected_revision(d.pop("expected_revision", UNSET))

        apply_control_changes_request = cls(
            changes=changes,
            surface_id=surface_id,
            dry_run=dry_run,
            expected_revision=expected_revision,
        )

        apply_control_changes_request.additional_properties = d
        return apply_control_changes_request

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
