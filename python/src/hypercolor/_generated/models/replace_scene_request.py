from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from typing_extensions import Self

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.replace_scene_request_metadata import ReplaceSceneRequestMetadata
    from ..models.replace_scene_request_transition import ReplaceSceneRequestTransition
    from ..models.replace_zone_request import ReplaceZoneRequest


T = TypeVar("T", bound="ReplaceSceneRequest")


@_attrs_define
class ReplaceSceneRequest:
    """Whole-document replacement body for `PUT /api/v1/scenes/{id}`.

    Accepts the [`SceneDocument`] a `GET` returned, edited in place. The
    server-owned fields it echoes are checked rather than written: `id`,
    `kind`, and `is_default` must match the stored scene, and `revision`
    is an optimistic-concurrency precondition with the same meaning as
    `If-Match`.

        Attributes:
            enabled (bool):
            kind (str):
            name (str):
            priority (int):
            transition (ReplaceSceneRequestTransition):
            activation_brightness (float | None | Unset):
            description (None | str | Unset):
            id (None | str | Unset):
            is_default (bool | None | Unset): Echo of the read-only default-scene flag. Must match the stored
                scene when present; it can never be changed.
            layout_id (None | str | Unset):
            metadata (ReplaceSceneRequestMetadata | Unset):
            mutation_mode (str | Unset):
            revision (int | None | Unset): The scene `revision` this replacement was built from. When
                present, a stale value is refused with `412 precondition_failed`,
                exactly as a stale `If-Match` is.
            run_epoch (None | str | Unset): Echo of the daemon run the read came from, accepted so a read can
                be written back as it arrived. Never checked: revisions never
                repeat across runs, so a `revision` from an earlier run is already
                refused.
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
    is_default: bool | None | Unset = UNSET
    layout_id: None | str | Unset = UNSET
    metadata: ReplaceSceneRequestMetadata | Unset = UNSET
    mutation_mode: str | Unset = UNSET
    revision: int | None | Unset = UNSET
    run_epoch: None | str | Unset = UNSET
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

        is_default: bool | None | Unset
        if isinstance(self.is_default, Unset):
            is_default = UNSET
        else:
            is_default = self.is_default

        layout_id: None | str | Unset
        if isinstance(self.layout_id, Unset):
            layout_id = UNSET
        else:
            layout_id = self.layout_id

        metadata: dict[str, Any] | Unset = UNSET
        if not isinstance(self.metadata, Unset):
            metadata = self.metadata.to_dict()

        mutation_mode = self.mutation_mode

        revision: int | None | Unset
        if isinstance(self.revision, Unset):
            revision = UNSET
        else:
            revision = self.revision

        run_epoch: None | str | Unset
        if isinstance(self.run_epoch, Unset):
            run_epoch = UNSET
        else:
            run_epoch = self.run_epoch

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
        if is_default is not UNSET:
            field_dict["is_default"] = is_default
        if layout_id is not UNSET:
            field_dict["layout_id"] = layout_id
        if metadata is not UNSET:
            field_dict["metadata"] = metadata
        if mutation_mode is not UNSET:
            field_dict["mutation_mode"] = mutation_mode
        if revision is not UNSET:
            field_dict["revision"] = revision
        if run_epoch is not UNSET:
            field_dict["run_epoch"] = run_epoch
        if unassigned_behavior is not UNSET:
            field_dict["unassigned_behavior"] = unassigned_behavior
        if zones is not UNSET:
            field_dict["zones"] = zones

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.replace_scene_request_metadata import (
            ReplaceSceneRequestMetadata,
        )
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

        def _parse_is_default(data: object) -> bool | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(bool | None | Unset, data)

        is_default = _parse_is_default(d.pop("is_default", UNSET))

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

        def _parse_revision(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        revision = _parse_revision(d.pop("revision", UNSET))

        def _parse_run_epoch(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        run_epoch = _parse_run_epoch(d.pop("run_epoch", UNSET))

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
            is_default=is_default,
            layout_id=layout_id,
            metadata=metadata,
            mutation_mode=mutation_mode,
            revision=revision,
            run_epoch=run_epoch,
            unassigned_behavior=unassigned_behavior,
            zones=zones,
        )

        return replace_scene_request
