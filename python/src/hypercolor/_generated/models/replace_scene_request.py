from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.replace_scene_request_metadata import ReplaceSceneRequestMetadata
    from ..models.replace_scene_request_transition import ReplaceSceneRequestTransition
    from ..models.replace_zone_request import ReplaceZoneRequest


T = TypeVar("T", bound="ReplaceSceneRequest")


@_attrs_define
class ReplaceSceneRequest:
    """Whole-document replacement body for `PUT /api/v1/scenes/{id}`.

    Attributes:
        enabled (bool):
        kind (str):
        name (str):
        priority (int):
        transition (ReplaceSceneRequestTransition):
        activation_brightness (float | None | Unset):
        description (None | str | Unset):
        id (None | str | Unset):
        layout_id (None | str | Unset):
        metadata (ReplaceSceneRequestMetadata | Unset):
        mutation_mode (str | Unset):
        unassigned_behavior (str | Unset):
        zones (list[ReplaceZoneRequest] | Unset):
    """

    enabled: bool
    kind: str
    name: str
    priority: int
    transition: ReplaceSceneRequestTransition
    activation_brightness: float | None | Unset = UNSET
    description: None | str | Unset = UNSET
    id: None | str | Unset = UNSET
    layout_id: None | str | Unset = UNSET
    metadata: ReplaceSceneRequestMetadata | Unset = UNSET
    mutation_mode: str | Unset = UNSET
    unassigned_behavior: str | Unset = UNSET
    zones: list[ReplaceZoneRequest] | Unset = UNSET

    def to_dict(self) -> dict[str, Any]:
        enabled = self.enabled

        kind = self.kind

        name = self.name

        priority = self.priority

        transition = self.transition.to_dict()

        activation_brightness: float | None | Unset
        if isinstance(self.activation_brightness, Unset):
            activation_brightness = UNSET
        else:
            activation_brightness = self.activation_brightness

        description: None | str | Unset
        if isinstance(self.description, Unset):
            description = UNSET
        else:
            description = self.description

        id: None | str | Unset
        if isinstance(self.id, Unset):
            id = UNSET
        else:
            id = self.id

        layout_id: None | str | Unset
        if isinstance(self.layout_id, Unset):
            layout_id = UNSET
        else:
            layout_id = self.layout_id

        metadata: dict[str, Any] | Unset = UNSET
        if not isinstance(self.metadata, Unset):
            metadata = self.metadata.to_dict()

        mutation_mode = self.mutation_mode

        unassigned_behavior = self.unassigned_behavior

        zones: list[dict[str, Any]] | Unset = UNSET
        if not isinstance(self.zones, Unset):
            zones = []
            for zones_item_data in self.zones:
                zones_item = zones_item_data.to_dict()
                zones.append(zones_item)

        field_dict: dict[str, Any] = {}

        field_dict.update(
            {
                "enabled": enabled,
                "kind": kind,
                "name": name,
                "priority": priority,
                "transition": transition,
            }
        )
        if activation_brightness is not UNSET:
            field_dict["activation_brightness"] = activation_brightness
        if description is not UNSET:
            field_dict["description"] = description
        if id is not UNSET:
            field_dict["id"] = id
        if layout_id is not UNSET:
            field_dict["layout_id"] = layout_id
        if metadata is not UNSET:
            field_dict["metadata"] = metadata
        if mutation_mode is not UNSET:
            field_dict["mutation_mode"] = mutation_mode
        if unassigned_behavior is not UNSET:
            field_dict["unassigned_behavior"] = unassigned_behavior
        if zones is not UNSET:
            field_dict["zones"] = zones

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.replace_scene_request_metadata import ReplaceSceneRequestMetadata
        from ..models.replace_scene_request_transition import (
            ReplaceSceneRequestTransition,
        )
        from ..models.replace_zone_request import ReplaceZoneRequest

        d = dict(src_dict)
        enabled = d.pop("enabled")

        kind = d.pop("kind")

        name = d.pop("name")

        priority = d.pop("priority")

        transition = ReplaceSceneRequestTransition.from_dict(d.pop("transition"))

        def _parse_activation_brightness(data: object) -> float | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(float | None | Unset, data)

        activation_brightness = _parse_activation_brightness(
            d.pop("activation_brightness", UNSET)
        )

        def _parse_description(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        description = _parse_description(d.pop("description", UNSET))

        def _parse_id(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        id = _parse_id(d.pop("id", UNSET))

        def _parse_layout_id(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        layout_id = _parse_layout_id(d.pop("layout_id", UNSET))

        _metadata = d.pop("metadata", UNSET)
        metadata: ReplaceSceneRequestMetadata | Unset
        if isinstance(_metadata, Unset):
            metadata = UNSET
        else:
            metadata = ReplaceSceneRequestMetadata.from_dict(_metadata)

        mutation_mode = d.pop("mutation_mode", UNSET)

        unassigned_behavior = d.pop("unassigned_behavior", UNSET)

        _zones = d.pop("zones", UNSET)
        zones: list[ReplaceZoneRequest] | Unset = UNSET
        if _zones is not UNSET:
            zones = []
            for zones_item_data in _zones:
                zones_item = ReplaceZoneRequest.from_dict(zones_item_data)

                zones.append(zones_item)

        replace_scene_request = cls(
            enabled=enabled,
            kind=kind,
            name=name,
            priority=priority,
            transition=transition,
            activation_brightness=activation_brightness,
            description=description,
            id=id,
            layout_id=layout_id,
            metadata=metadata,
            mutation_mode=mutation_mode,
            unassigned_behavior=unassigned_behavior,
            zones=zones,
        )

        return replace_scene_request
