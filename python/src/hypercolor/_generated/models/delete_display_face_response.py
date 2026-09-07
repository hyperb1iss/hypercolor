from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.display_face_scope import DisplayFaceScope
from ..types import UNSET, Unset

T = TypeVar("T", bound="DeleteDisplayFaceResponse")


@_attrs_define
class DeleteDisplayFaceResponse:
    """Response from `DELETE /api/v1/displays/{id}/face`.

    Attributes:
        deleted (bool):
        device_id (str):
        scope (DisplayFaceScope): Which assignment layer a face operation targets (spec 69 §3.6).

            `default` persists across scenes (the display's own face); `scene`
            writes into the active scene's display zone, which always wins while
            that scene is active.
        scene_id (None | str | Unset):
    """

    deleted: bool
    device_id: str
    scope: DisplayFaceScope
    scene_id: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        deleted = self.deleted

        device_id = self.device_id

        scope = self.scope.value

        scene_id: None | str | Unset
        if isinstance(self.scene_id, Unset):
            scene_id = UNSET
        else:
            scene_id = self.scene_id

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "deleted": deleted,
                "device_id": device_id,
                "scope": scope,
            }
        )
        if scene_id is not UNSET:
            field_dict["scene_id"] = scene_id

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        deleted = d.pop("deleted")

        device_id = d.pop("device_id")

        scope = DisplayFaceScope(d.pop("scope"))

        def _parse_scene_id(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        scene_id = _parse_scene_id(d.pop("scene_id", UNSET))

        delete_display_face_response = cls(
            deleted=deleted,
            device_id=device_id,
            scope=scope,
            scene_id=scene_id,
        )

        delete_display_face_response.additional_properties = d
        return delete_display_face_response

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
