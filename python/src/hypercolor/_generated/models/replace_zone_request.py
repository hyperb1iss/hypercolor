from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.replace_scene_layer_request import ReplaceSceneLayerRequest
    from ..models.replace_zone_request_display_target_type_0 import (
        ReplaceZoneRequestDisplayTargetType0,
    )
    from ..models.zone_layout_resource import ZoneLayoutResource
    from ..models.zone_member import ZoneMember


T = TypeVar("T", bound="ReplaceZoneRequest")


@_attrs_define
class ReplaceZoneRequest:
    """
    Attributes:
        brightness (float):
        enabled (bool):
        name (str):
        color (None | str | Unset):
        description (None | str | Unset):
        display_target (None | ReplaceZoneRequestDisplayTargetType0 | Unset):
        id (None | str | Unset):
        layers (list[ReplaceSceneLayerRequest] | Unset):
        layout (None | Unset | ZoneLayoutResource):
        members (list[ZoneMember] | Unset):
        role (str | Unset):
    """

    brightness: float
    enabled: bool
    name: str
    color: None | str | Unset = UNSET
    description: None | str | Unset = UNSET
    display_target: None | ReplaceZoneRequestDisplayTargetType0 | Unset = UNSET
    id: None | str | Unset = UNSET
    layers: list[ReplaceSceneLayerRequest] | Unset = UNSET
    layout: None | Unset | ZoneLayoutResource = UNSET
    members: list[ZoneMember] | Unset = UNSET
    role: str | Unset = UNSET

    def to_dict(self) -> dict[str, Any]:
        from ..models.replace_zone_request_display_target_type_0 import (
            ReplaceZoneRequestDisplayTargetType0,
        )
        from ..models.zone_layout_resource import ZoneLayoutResource

        brightness = self.brightness

        enabled = self.enabled

        name = self.name

        color: None | str | Unset
        if isinstance(self.color, Unset):
            color = UNSET
        else:
            color = self.color

        description: None | str | Unset
        if isinstance(self.description, Unset):
            description = UNSET
        else:
            description = self.description

        display_target: dict[str, Any] | None | Unset
        if isinstance(self.display_target, Unset):
            display_target = UNSET
        elif isinstance(self.display_target, ReplaceZoneRequestDisplayTargetType0):
            display_target = self.display_target.to_dict()
        else:
            display_target = self.display_target

        id: None | str | Unset
        if isinstance(self.id, Unset):
            id = UNSET
        else:
            id = self.id

        layers: list[dict[str, Any]] | Unset = UNSET
        if not isinstance(self.layers, Unset):
            layers = []
            for layers_item_data in self.layers:
                layers_item = layers_item_data.to_dict()
                layers.append(layers_item)

        layout: dict[str, Any] | None | Unset
        if isinstance(self.layout, Unset):
            layout = UNSET
        elif isinstance(self.layout, ZoneLayoutResource):
            layout = self.layout.to_dict()
        else:
            layout = self.layout

        members: list[dict[str, Any]] | Unset = UNSET
        if not isinstance(self.members, Unset):
            members = []
            for members_item_data in self.members:
                members_item = members_item_data.to_dict()
                members.append(members_item)

        role = self.role

        field_dict: dict[str, Any] = {}

        field_dict.update(
            {
                "brightness": brightness,
                "enabled": enabled,
                "name": name,
            }
        )
        if color is not UNSET:
            field_dict["color"] = color
        if description is not UNSET:
            field_dict["description"] = description
        if display_target is not UNSET:
            field_dict["display_target"] = display_target
        if id is not UNSET:
            field_dict["id"] = id
        if layers is not UNSET:
            field_dict["layers"] = layers
        if layout is not UNSET:
            field_dict["layout"] = layout
        if members is not UNSET:
            field_dict["members"] = members
        if role is not UNSET:
            field_dict["role"] = role

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.replace_scene_layer_request import ReplaceSceneLayerRequest
        from ..models.replace_zone_request_display_target_type_0 import (
            ReplaceZoneRequestDisplayTargetType0,
        )
        from ..models.zone_layout_resource import ZoneLayoutResource
        from ..models.zone_member import ZoneMember

        d = dict(src_dict)
        brightness = d.pop("brightness")

        enabled = d.pop("enabled")

        name = d.pop("name")

        def _parse_color(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        color = _parse_color(d.pop("color", UNSET))

        def _parse_description(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        description = _parse_description(d.pop("description", UNSET))

        def _parse_display_target(
            data: object,
        ) -> None | ReplaceZoneRequestDisplayTargetType0 | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                display_target_type_0 = ReplaceZoneRequestDisplayTargetType0.from_dict(
                    data
                )

                return display_target_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | ReplaceZoneRequestDisplayTargetType0 | Unset, data)

        display_target = _parse_display_target(d.pop("display_target", UNSET))

        def _parse_id(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        id = _parse_id(d.pop("id", UNSET))

        _layers = d.pop("layers", UNSET)
        layers: list[ReplaceSceneLayerRequest] | Unset = UNSET
        if _layers is not UNSET:
            layers = []
            for layers_item_data in _layers:
                layers_item = ReplaceSceneLayerRequest.from_dict(layers_item_data)

                layers.append(layers_item)

        def _parse_layout(data: object) -> None | Unset | ZoneLayoutResource:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                layout_type_1 = ZoneLayoutResource.from_dict(data)

                return layout_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | Unset | ZoneLayoutResource, data)

        layout = _parse_layout(d.pop("layout", UNSET))

        _members = d.pop("members", UNSET)
        members: list[ZoneMember] | Unset = UNSET
        if _members is not UNSET:
            members = []
            for members_item_data in _members:
                members_item = ZoneMember.from_dict(members_item_data)

                members.append(members_item)

        role = d.pop("role", UNSET)

        replace_zone_request = cls(
            brightness=brightness,
            enabled=enabled,
            name=name,
            color=color,
            description=description,
            display_target=display_target,
            id=id,
            layers=layers,
            layout=layout,
            members=members,
            role=role,
        )

        return replace_zone_request
