from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from typing_extensions import Self

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.layer_move import LayerMove
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
        layer_moves (list[LayerMove] | None | Unset): The scoped layers to move with the outputs, replayed from a
            receipt's `layer_moves` beside its `changes`.

            Omitted, the daemon decides which scoped layers follow the moved
            outputs. Present, exactly these moves are applied and nothing else
            is carried, so replaying a receipt's reversed moves restores each
            layer's zone, stack position, and scope. Only valid with explicit
            `changes`.
    """

    scene_id: str
    assignment: MemberAssignmentTarget | None | Unset = UNSET
    changes: list[MemberEdit] | Unset = UNSET
    layer_moves: list[LayerMove] | None | Unset = UNSET

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

        layer_moves: list[dict[str, Any]] | None | Unset
        if isinstance(self.layer_moves, Unset):
            layer_moves = UNSET
        elif isinstance(self.layer_moves, list):
            layer_moves = []
            for layer_moves_type_0_item_data in self.layer_moves:
                layer_moves_type_0_item = layer_moves_type_0_item_data.to_dict()
                layer_moves.append(layer_moves_type_0_item)

        else:
            layer_moves = self.layer_moves

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
        if layer_moves is not UNSET:
            field_dict["layer_moves"] = layer_moves

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.layer_move import LayerMove
        from ..models.member_assignment_target import (
            MemberAssignmentTarget,
        )
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
                assignment_type_0 = MemberAssignmentTarget.from_dict(data)

                return assignment_type_0
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

        def _parse_layer_moves(data: object) -> list[LayerMove] | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, list):
                    raise TypeError()
                layer_moves_type_0 = []
                _layer_moves_type_0 = data
                for layer_moves_type_0_item_data in _layer_moves_type_0:
                    layer_moves_type_0_item = LayerMove.from_dict(
                        layer_moves_type_0_item_data
                    )

                    layer_moves_type_0.append(layer_moves_type_0_item)

                return layer_moves_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(list[LayerMove] | None | Unset, data)

        layer_moves = _parse_layer_moves(d.pop("layer_moves", UNSET))

        edit_members_request = cls(
            scene_id=scene_id,
            assignment=assignment,
            changes=changes,
            layer_moves=layer_moves,
        )

        return edit_members_request
