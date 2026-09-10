from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
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
    """

    changes: list[MemberEdit]
    document: SceneDocument
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        changes = []
        for changes_item_data in self.changes:
            changes_item = changes_item_data.to_dict()
            changes.append(changes_item)

        document = self.document.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "changes": changes,
                "document": document,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.member_edit import MemberEdit
        from ..models.scene_document import SceneDocument

        d = dict(src_dict)
        changes = []
        _changes = d.pop("changes")
        for changes_item_data in _changes:
            changes_item = MemberEdit.from_dict(changes_item_data)

            changes.append(changes_item)

        document = SceneDocument.from_dict(d.pop("document"))

        edit_members_response = cls(
            changes=changes,
            document=document,
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
