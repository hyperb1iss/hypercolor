from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="UpdateDisplayFaceCompositionRequest")


@_attrs_define
class UpdateDisplayFaceCompositionRequest:
    """Request body for `PATCH /api/v1/displays/{id}/face/composition`.

    Attributes:
        blend_mode (None | str | Unset):
        opacity (float | None | Unset):
    """

    blend_mode: None | str | Unset = UNSET
    opacity: float | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        blend_mode: None | str | Unset
        if isinstance(self.blend_mode, Unset):
            blend_mode = UNSET
        else:
            blend_mode = self.blend_mode

        opacity: float | None | Unset
        if isinstance(self.opacity, Unset):
            opacity = UNSET
        else:
            opacity = self.opacity

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if blend_mode is not UNSET:
            field_dict["blend_mode"] = blend_mode
        if opacity is not UNSET:
            field_dict["opacity"] = opacity

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)

        def _parse_blend_mode(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        blend_mode = _parse_blend_mode(d.pop("blend_mode", UNSET))

        def _parse_opacity(data: object) -> float | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(float | None | Unset, data)

        opacity = _parse_opacity(d.pop("opacity", UNSET))

        update_display_face_composition_request = cls(
            blend_mode=blend_mode,
            opacity=opacity,
        )

        update_display_face_composition_request.additional_properties = d
        return update_display_face_composition_request

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
