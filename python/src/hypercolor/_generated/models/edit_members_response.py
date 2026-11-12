from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.layer_move import LayerMove
    from ..models.member_edit import MemberEdit
    from ..models.scene_document import SceneDocument


T = TypeVar("T", bound="EditMembersResponse")


@_attrs_define
class EditMembersResponse:
    """Committed scene and reversible, canonical membership changes.

    Attributes:
        changes (list[MemberEdit]):
        document (SceneDocument): The `GET /scene` document: the full live tree.

            Always present — an active scene always exists (Spec 78 §1.1), so
            there is no idle sentinel and no all-optional shape.
        layer_moves (list[LayerMove] | Unset): Every scoped layer the edit moved or narrowed, in the order a
            replay applies them.
    """

    changes: list[MemberEdit]
    document: SceneDocument
    layer_moves: list[LayerMove] | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        changes = []
        for changes_item_data in self.changes:
            changes_item = changes_item_data.to_dict()
            changes.append(changes_item)

        document = self.document.to_dict()

        layer_moves: list[dict[str, Any]] | Unset = UNSET
        if not isinstance(self.layer_moves, Unset):
            layer_moves = []
            for layer_moves_item_data in self.layer_moves:
                layer_moves_item = layer_moves_item_data.to_dict()
                layer_moves.append(layer_moves_item)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "changes": changes,
                "document": document,
            }
        )
        if layer_moves is not UNSET:
            field_dict["layer_moves"] = layer_moves

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.layer_move import LayerMove
        from ..models.member_edit import MemberEdit
        from ..models.scene_document import SceneDocument

        d = dict(src_dict)
        changes = []
        _changes = d.pop("changes")
        for changes_item_data in _changes:
            changes_item = MemberEdit.from_dict(changes_item_data)

            changes.append(changes_item)

        document = SceneDocument.from_dict(d.pop("document"))

        _layer_moves = d.pop("layer_moves", UNSET)
        layer_moves: list[LayerMove] | Unset = UNSET
        if _layer_moves is not UNSET:
            layer_moves = []
            for layer_moves_item_data in _layer_moves:
                layer_moves_item = LayerMove.from_dict(layer_moves_item_data)

                layer_moves.append(layer_moves_item)

        edit_members_response = cls(
            changes=changes,
            document=document,
            layer_moves=layer_moves,
        )

        edit_members_response.additional_properties = d
        return edit_members_response

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
