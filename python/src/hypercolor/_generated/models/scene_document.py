from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.scene_document_metadata import SceneDocumentMetadata
    from ..models.scene_document_transition import SceneDocumentTransition
    from ..models.zone_resource import ZoneResource


T = TypeVar("T", bound="SceneDocument")


@_attrs_define
class SceneDocument:
    """The `GET /scene` document: the full live tree.

    Always present — an active scene always exists (Spec 78 §1.1), so
    there is no idle sentinel and no all-optional shape.

        Attributes:
            id (str):
            is_default (bool): Whether this is the auto-managed default scene, which cannot be
                renamed or deleted.
            kind (str):
            name (str):
            revision (int): The commit generation. Served as `ETag`; the one wire version
                token (Spec 78 §1.6).
            zones (list[ZoneResource]): Every authored zone with its full stack, declaration order.
            activation_brightness (float | None | Unset):
            description (None | str | Unset):
            enabled (bool | Unset):
            layout_id (None | str | Unset):
            metadata (SceneDocumentMetadata | Unset):
            mutation_mode (str | Unset):
            priority (int | Unset):
            transition (SceneDocumentTransition | Unset):
            unassigned_behavior (str | Unset):
    """

    id: str
    is_default: bool
    kind: str
    name: str
    revision: int
    zones: list[ZoneResource]
    activation_brightness: float | None | Unset = UNSET
    description: None | str | Unset = UNSET
    enabled: bool | Unset = UNSET
    layout_id: None | str | Unset = UNSET
    metadata: SceneDocumentMetadata | Unset = UNSET
    mutation_mode: str | Unset = UNSET
    priority: int | Unset = UNSET
    transition: SceneDocumentTransition | Unset = UNSET
    unassigned_behavior: str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        id = self.id

        is_default = self.is_default

        kind = self.kind

        name = self.name

        revision = self.revision

        zones = []
        for zones_item_data in self.zones:
            zones_item = zones_item_data.to_dict()
            zones.append(zones_item)

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

        enabled = self.enabled

        layout_id: None | str | Unset
        if isinstance(self.layout_id, Unset):
            layout_id = UNSET
        else:
            layout_id = self.layout_id

        metadata: dict[str, Any] | Unset = UNSET
        if not isinstance(self.metadata, Unset):
            metadata = self.metadata.to_dict()

        mutation_mode = self.mutation_mode

        priority = self.priority

        transition: dict[str, Any] | Unset = UNSET
        if not isinstance(self.transition, Unset):
            transition = self.transition.to_dict()

        unassigned_behavior = self.unassigned_behavior

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "id": id,
                "is_default": is_default,
                "kind": kind,
                "name": name,
                "revision": revision,
                "zones": zones,
            }
        )
        if activation_brightness is not UNSET:
            field_dict["activation_brightness"] = activation_brightness
        if description is not UNSET:
            field_dict["description"] = description
        if enabled is not UNSET:
            field_dict["enabled"] = enabled
        if layout_id is not UNSET:
            field_dict["layout_id"] = layout_id
        if metadata is not UNSET:
            field_dict["metadata"] = metadata
        if mutation_mode is not UNSET:
            field_dict["mutation_mode"] = mutation_mode
        if priority is not UNSET:
            field_dict["priority"] = priority
        if transition is not UNSET:
            field_dict["transition"] = transition
        if unassigned_behavior is not UNSET:
            field_dict["unassigned_behavior"] = unassigned_behavior

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.scene_document_metadata import SceneDocumentMetadata
        from ..models.scene_document_transition import SceneDocumentTransition
        from ..models.zone_resource import ZoneResource

        d = dict(src_dict)
        id = d.pop("id")

        is_default = d.pop("is_default")

        kind = d.pop("kind")

        name = d.pop("name")

        revision = d.pop("revision")

        zones = []
        _zones = d.pop("zones")
        for zones_item_data in _zones:
            zones_item = ZoneResource.from_dict(zones_item_data)

            zones.append(zones_item)

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

        enabled = d.pop("enabled", UNSET)

        def _parse_layout_id(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        layout_id = _parse_layout_id(d.pop("layout_id", UNSET))

        _metadata = d.pop("metadata", UNSET)
        metadata: SceneDocumentMetadata | Unset
        if isinstance(_metadata, Unset):
            metadata = UNSET
        else:
            metadata = SceneDocumentMetadata.from_dict(_metadata)

        mutation_mode = d.pop("mutation_mode", UNSET)

        priority = d.pop("priority", UNSET)

        _transition = d.pop("transition", UNSET)
        transition: SceneDocumentTransition | Unset
        if isinstance(_transition, Unset):
            transition = UNSET
        else:
            transition = SceneDocumentTransition.from_dict(_transition)

        unassigned_behavior = d.pop("unassigned_behavior", UNSET)

        scene_document = cls(
            id=id,
            is_default=is_default,
            kind=kind,
            name=name,
            revision=revision,
            zones=zones,
            activation_brightness=activation_brightness,
            description=description,
            enabled=enabled,
            layout_id=layout_id,
            metadata=metadata,
            mutation_mode=mutation_mode,
            priority=priority,
            transition=transition,
            unassigned_behavior=unassigned_behavior,
        )

        scene_document.additional_properties = d
        return scene_document

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
