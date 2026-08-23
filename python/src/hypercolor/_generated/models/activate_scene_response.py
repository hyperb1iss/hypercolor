from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.activated_scene_ref import ActivatedSceneRef
    from ..models.scene_layout_activation_outcome import SceneLayoutActivationOutcome
    from ..models.side_effect_outcome import SideEffectOutcome


T = TypeVar("T", bound="ActivateSceneResponse")


@_attrs_define
class ActivateSceneResponse:
    """Response for `POST /api/v1/scenes/{id}/activate`.

    Attributes:
        activated (bool):
        brightness (SideEffectOutcome): One post-commit side-effect outcome (Spec 78 §2.3, §3.2): the
            commit stands, the outcome says whether the side effect landed,
            and a failure carries its reason.
        layout (SceneLayoutActivationOutcome): Post-commit outcome for a scene's optional named layout.
        scene (ActivatedSceneRef): The scene an activation resolved to, by id and name.
    """

    activated: bool
    brightness: SideEffectOutcome
    layout: SceneLayoutActivationOutcome
    scene: ActivatedSceneRef
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        activated = self.activated

        brightness = self.brightness.to_dict()

        layout = self.layout.to_dict()

        scene = self.scene.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "activated": activated,
                "brightness": brightness,
                "layout": layout,
                "scene": scene,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.activated_scene_ref import ActivatedSceneRef
        from ..models.scene_layout_activation_outcome import (
            SceneLayoutActivationOutcome,
        )
        from ..models.side_effect_outcome import SideEffectOutcome

        d = dict(src_dict)
        activated = d.pop("activated")

        brightness = SideEffectOutcome.from_dict(d.pop("brightness"))

        layout = SceneLayoutActivationOutcome.from_dict(d.pop("layout"))

        scene = ActivatedSceneRef.from_dict(d.pop("scene"))

        activate_scene_response = cls(
            activated=activated,
            brightness=brightness,
            layout=layout,
            scene=scene,
        )

        activate_scene_response.additional_properties = d
        return activate_scene_response

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
