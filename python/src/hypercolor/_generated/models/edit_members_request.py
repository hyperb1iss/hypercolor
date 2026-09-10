from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.member_assignment_target import MemberAssignmentTarget
    from ..models.member_edit import MemberEdit


T = TypeVar("T", bound="EditMembersRequest")


@_attrs_define
class EditMembersRequest:
    """`POST /scene/members/edit`: one revision-fenced membership transaction.

    Attributes:
        scene_id (str):
        assignment (MemberAssignmentTarget | None | Unset):
        changes (list[MemberEdit] | Unset):
    """

    scene_id: str
    assignment: MemberAssignmentTarget | None | Unset = UNSET
    changes: list[MemberEdit] | Unset = UNSET

    def to_dict(self) -> dict[str, Any]:
        from ..models.member_assignment_target import MemberAssignmentTarget

        scene_id = self.scene_id

        assignment: dict[str, Any] | None | Unset
        if isinstance(self.assignment, Unset):
            assignment = UNSET
        elif isinstance(self.assignment, MemberAssignmentTarget):
            assignment = self.assignment.to_dict()
        else:
            assignment = self.assignment

        changes: list[dict[str, Any]] | Unset = UNSET
        if not isinstance(self.changes, Unset):
            changes = []
            for changes_item_data in self.changes:
                changes_item = changes_item_data.to_dict()
                changes.append(changes_item)

        field_dict: dict[str, Any] = {}

        field_dict.update(
            {
                "scene_id": scene_id,
            }
        )
        if assignment is not UNSET:
            field_dict["assignment"] = assignment
        if changes is not UNSET:
            field_dict["changes"] = changes

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.member_assignment_target import MemberAssignmentTarget
        from ..models.member_edit import MemberEdit

        d = dict(src_dict)
        scene_id = d.pop("scene_id")

        def _parse_assignment(data: object) -> MemberAssignmentTarget | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                assignment_type_1 = MemberAssignmentTarget.from_dict(data)

                return assignment_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(MemberAssignmentTarget | None | Unset, data)

        assignment = _parse_assignment(d.pop("assignment", UNSET))

        _changes = d.pop("changes", UNSET)
        changes: list[MemberEdit] | Unset = UNSET
        if _changes is not UNSET:
            changes = []
            for changes_item_data in _changes:
                changes_item = MemberEdit.from_dict(changes_item_data)

                changes.append(changes_item)

        edit_members_request = cls(
            scene_id=scene_id,
            assignment=assignment,
            changes=changes,
        )

        return edit_members_request
