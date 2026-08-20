from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.zone_layout_resource import ZoneLayoutResource
    from ..models.zone_member import ZoneMember
    from ..models.zone_resource_display_target_type_0 import (
        ZoneResourceDisplayTargetType0,
    )
    from ..models.zone_resource_layers_item import ZoneResourceLayersItem


T = TypeVar("T", bound="ZoneResource")


@_attrs_define
class ZoneResource:
    """One authored zone inside the live document (Spec 78 §1.3).

    Attributes:
        brightness (float):
        enabled (bool):
        id (str):
        layers (list[ZoneResourceLayersItem]): The authored bottom-to-top layer stack. Layers are the real,
            addressable unit: clients patch the layer id they read here.
        members (list[ZoneMember]): Device segments assigned to this zone, addressed by membership
            id (Spec 78 §1.2) — never by device-scoped segment name.
        name (str):
        color (None | str | Unset):
        description (None | str | Unset):
        display_target (None | Unset | ZoneResourceDisplayTargetType0): Present on Display-role zones only.
        layout (None | Unset | ZoneLayoutResource):
        role (str | Unset):
    """

    brightness: float
    enabled: bool
    id: str
    layers: list[ZoneResourceLayersItem]
    members: list[ZoneMember]
    name: str
    color: None | str | Unset = UNSET
    description: None | str | Unset = UNSET
    display_target: None | Unset | ZoneResourceDisplayTargetType0 = UNSET
    layout: None | Unset | ZoneLayoutResource = UNSET
    role: str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.zone_layout_resource import ZoneLayoutResource
        from ..models.zone_resource_display_target_type_0 import (
            ZoneResourceDisplayTargetType0,
        )

        brightness = self.brightness

        enabled = self.enabled

        id = self.id

        layers = []
        for layers_item_data in self.layers:
            layers_item = layers_item_data.to_dict()
            layers.append(layers_item)

        members = []
        for members_item_data in self.members:
            members_item = members_item_data.to_dict()
            members.append(members_item)

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
        elif isinstance(self.display_target, ZoneResourceDisplayTargetType0):
            display_target = self.display_target.to_dict()
        else:
            display_target = self.display_target

        layout: dict[str, Any] | None | Unset
        if isinstance(self.layout, Unset):
            layout = UNSET
        elif isinstance(self.layout, ZoneLayoutResource):
            layout = self.layout.to_dict()
        else:
            layout = self.layout

        role = self.role

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "brightness": brightness,
                "enabled": enabled,
                "id": id,
                "layers": layers,
                "members": members,
                "name": name,
            }
        )
        if color is not UNSET:
            field_dict["color"] = color
        if description is not UNSET:
            field_dict["description"] = description
        if display_target is not UNSET:
            field_dict["display_target"] = display_target
        if layout is not UNSET:
            field_dict["layout"] = layout
        if role is not UNSET:
            field_dict["role"] = role

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.zone_layout_resource import ZoneLayoutResource
        from ..models.zone_member import ZoneMember
        from ..models.zone_resource_display_target_type_0 import (
            ZoneResourceDisplayTargetType0,
        )
        from ..models.zone_resource_layers_item import ZoneResourceLayersItem

        d = dict(src_dict)
        brightness = d.pop("brightness")

        enabled = d.pop("enabled")

        id = d.pop("id")

        layers = []
        _layers = d.pop("layers")
        for layers_item_data in _layers:
            layers_item = ZoneResourceLayersItem.from_dict(layers_item_data)

            layers.append(layers_item)

        members = []
        _members = d.pop("members")
        for members_item_data in _members:
            members_item = ZoneMember.from_dict(members_item_data)

            members.append(members_item)

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
        ) -> None | Unset | ZoneResourceDisplayTargetType0:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                display_target_type_0 = ZoneResourceDisplayTargetType0.from_dict(data)

                return display_target_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | Unset | ZoneResourceDisplayTargetType0, data)

        display_target = _parse_display_target(d.pop("display_target", UNSET))

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

        role = d.pop("role", UNSET)

        zone_resource = cls(
            brightness=brightness,
            enabled=enabled,
            id=id,
            layers=layers,
            members=members,
            name=name,
            color=color,
            description=description,
            display_target=display_target,
            layout=layout,
            role=role,
        )

        zone_resource.additional_properties = d
        return zone_resource

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
