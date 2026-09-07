from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.blend_mode import BlendMode
from ..models.display_face_scope import DisplayFaceScope
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.set_display_face_request_controls import SetDisplayFaceRequestControls


T = TypeVar("T", bound="SetDisplayFaceRequest")


@_attrs_define
class SetDisplayFaceRequest:
    """Request body for `PUT /api/v1/displays/{id}/face`.

    Attributes:
        effect_id (str):
        blend_mode (BlendMode | None | Unset):
        controls (SetDisplayFaceRequestControls | Unset):
        opacity (float | None | Unset):
        scope (DisplayFaceScope | Unset): Which assignment layer a face operation targets (spec 69 §3.6).

            `default` persists across scenes (the display's own face); `scene`
            writes into the active scene's display zone, which always wins while
            that scene is active.
    """

    effect_id: str
    blend_mode: BlendMode | None | Unset = UNSET
    controls: SetDisplayFaceRequestControls | Unset = UNSET
    opacity: float | None | Unset = UNSET
    scope: DisplayFaceScope | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        effect_id = self.effect_id

        blend_mode: None | str | Unset
        if isinstance(self.blend_mode, Unset):
            blend_mode = UNSET
        elif isinstance(self.blend_mode, BlendMode):
            blend_mode = self.blend_mode.value
        else:
            blend_mode = self.blend_mode

        controls: dict[str, Any] | Unset = UNSET
        if not isinstance(self.controls, Unset):
            controls = self.controls.to_dict()

        opacity: float | None | Unset
        if isinstance(self.opacity, Unset):
            opacity = UNSET
        else:
            opacity = self.opacity

        scope: str | Unset = UNSET
        if not isinstance(self.scope, Unset):
            scope = self.scope.value

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "effect_id": effect_id,
            }
        )
        if blend_mode is not UNSET:
            field_dict["blend_mode"] = blend_mode
        if controls is not UNSET:
            field_dict["controls"] = controls
        if opacity is not UNSET:
            field_dict["opacity"] = opacity
        if scope is not UNSET:
            field_dict["scope"] = scope

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.set_display_face_request_controls import (
            SetDisplayFaceRequestControls,
        )

        d = dict(src_dict)
        effect_id = d.pop("effect_id")

        def _parse_blend_mode(data: object) -> BlendMode | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                blend_mode_type_1 = BlendMode(data)

                return blend_mode_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(BlendMode | None | Unset, data)

        blend_mode = _parse_blend_mode(d.pop("blend_mode", UNSET))

        _controls = d.pop("controls", UNSET)
        controls: SetDisplayFaceRequestControls | Unset
        if isinstance(_controls, Unset):
            controls = UNSET
        else:
            controls = SetDisplayFaceRequestControls.from_dict(_controls)

        def _parse_opacity(data: object) -> float | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(float | None | Unset, data)

        opacity = _parse_opacity(d.pop("opacity", UNSET))

        _scope = d.pop("scope", UNSET)
        scope: DisplayFaceScope | Unset
        if isinstance(_scope, Unset):
            scope = UNSET
        else:
            scope = DisplayFaceScope(_scope)

        set_display_face_request = cls(
            effect_id=effect_id,
            blend_mode=blend_mode,
            controls=controls,
            opacity=opacity,
            scope=scope,
        )

        set_display_face_request.additional_properties = d
        return set_display_face_request

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
