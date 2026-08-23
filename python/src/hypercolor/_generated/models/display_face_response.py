from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.display_face_scope import DisplayFaceScope
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.display_face_response_zone import DisplayFaceResponseZone
    from ..models.effect_metadata import EffectMetadata


T = TypeVar("T", bound="DisplayFaceResponse")


@_attrs_define
class DisplayFaceResponse:
    """Response from `GET /api/v1/displays/{id}/face` and every face mutation
    route.

        Attributes:
            device_id (str):
            effect (EffectMetadata): Universal effect descriptor.

                Serialized as TOML for native effects and as JSON for the REST API
                and WebSocket protocol. This is the canonical metadata attached to
                every effect regardless of rendering path.
            scene_id (str):
            zone (DisplayFaceResponseZone):
            default_assigned (bool | Unset): Whether a persisted default face exists for this display.
            live_scope (DisplayFaceScope | Unset): Which assignment layer a face operation targets (spec 69 §3.6).

                `default` persists across scenes (the display's own face); `scene`
                writes into the active scene's display zone, which always wins while
                that scene is active.
            scene_assigned (bool | Unset): Whether the active scene has its own face assignment for this display.
    """

    device_id: str
    effect: EffectMetadata
    scene_id: str
    zone: DisplayFaceResponseZone
    default_assigned: bool | Unset = UNSET
    live_scope: DisplayFaceScope | Unset = UNSET
    scene_assigned: bool | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        device_id = self.device_id

        effect = self.effect.to_dict()

        scene_id = self.scene_id

        zone = self.zone.to_dict()

        default_assigned = self.default_assigned

        live_scope: str | Unset = UNSET
        if not isinstance(self.live_scope, Unset):
            live_scope = self.live_scope.value

        scene_assigned = self.scene_assigned

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "device_id": device_id,
                "effect": effect,
                "scene_id": scene_id,
                "zone": zone,
            }
        )
        if default_assigned is not UNSET:
            field_dict["default_assigned"] = default_assigned
        if live_scope is not UNSET:
            field_dict["live_scope"] = live_scope
        if scene_assigned is not UNSET:
            field_dict["scene_assigned"] = scene_assigned

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.display_face_response_zone import DisplayFaceResponseZone
        from ..models.effect_metadata import EffectMetadata

        d = dict(src_dict)
        device_id = d.pop("device_id")

        effect = EffectMetadata.from_dict(d.pop("effect"))

        scene_id = d.pop("scene_id")

        zone = DisplayFaceResponseZone.from_dict(d.pop("zone"))

        default_assigned = d.pop("default_assigned", UNSET)

        _live_scope = d.pop("live_scope", UNSET)
        live_scope: DisplayFaceScope | Unset
        if isinstance(_live_scope, Unset):
            live_scope = UNSET
        else:
            live_scope = DisplayFaceScope(_live_scope)

        scene_assigned = d.pop("scene_assigned", UNSET)

        display_face_response = cls(
            device_id=device_id,
            effect=effect,
            scene_id=scene_id,
            zone=zone,
            default_assigned=default_assigned,
            live_scope=live_scope,
            scene_assigned=scene_assigned,
        )

        display_face_response.additional_properties = d
        return display_face_response

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
