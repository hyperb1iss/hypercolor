from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.broadcast_media_layer_request_bindings_item import (
        BroadcastMediaLayerRequestBindingsItem,
    )
    from ..models.broadcast_media_layer_request_playback import (
        BroadcastMediaLayerRequestPlayback,
    )
    from ..models.broadcast_media_layer_request_targets_item import (
        BroadcastMediaLayerRequestTargetsItem,
    )


T = TypeVar("T", bound="BroadcastMediaLayerRequest")


@_attrs_define
class BroadcastMediaLayerRequest:
    """
    Attributes:
        asset_id (str):
        bindings (list[BroadcastMediaLayerRequestBindingsItem] | Unset):
        blend (str | Unset):
        enabled (bool | Unset):
        name (None | str | Unset):
        opacity (float | Unset):
        playback (BroadcastMediaLayerRequestPlayback | Unset):
        targets (list[BroadcastMediaLayerRequestTargetsItem] | Unset):
    """

    asset_id: str
    bindings: list[BroadcastMediaLayerRequestBindingsItem] | Unset = UNSET
    blend: str | Unset = UNSET
    enabled: bool | Unset = UNSET
    name: None | str | Unset = UNSET
    opacity: float | Unset = UNSET
    playback: BroadcastMediaLayerRequestPlayback | Unset = UNSET
    targets: list[BroadcastMediaLayerRequestTargetsItem] | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        asset_id = self.asset_id

        bindings: list[dict[str, Any]] | Unset = UNSET
        if not isinstance(self.bindings, Unset):
            bindings = []
            for bindings_item_data in self.bindings:
                bindings_item = bindings_item_data.to_dict()
                bindings.append(bindings_item)

        blend = self.blend

        enabled = self.enabled

        name: None | str | Unset
        if isinstance(self.name, Unset):
            name = UNSET
        else:
            name = self.name

        opacity = self.opacity

        playback: dict[str, Any] | Unset = UNSET
        if not isinstance(self.playback, Unset):
            playback = self.playback.to_dict()

        targets: list[dict[str, Any]] | Unset = UNSET
        if not isinstance(self.targets, Unset):
            targets = []
            for targets_item_data in self.targets:
                targets_item = targets_item_data.to_dict()
                targets.append(targets_item)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "asset_id": asset_id,
            }
        )
        if bindings is not UNSET:
            field_dict["bindings"] = bindings
        if blend is not UNSET:
            field_dict["blend"] = blend
        if enabled is not UNSET:
            field_dict["enabled"] = enabled
        if name is not UNSET:
            field_dict["name"] = name
        if opacity is not UNSET:
            field_dict["opacity"] = opacity
        if playback is not UNSET:
            field_dict["playback"] = playback
        if targets is not UNSET:
            field_dict["targets"] = targets

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.broadcast_media_layer_request_bindings_item import (
            BroadcastMediaLayerRequestBindingsItem,
        )
        from ..models.broadcast_media_layer_request_playback import (
            BroadcastMediaLayerRequestPlayback,
        )
        from ..models.broadcast_media_layer_request_targets_item import (
            BroadcastMediaLayerRequestTargetsItem,
        )

        d = dict(src_dict)
        asset_id = d.pop("asset_id")

        _bindings = d.pop("bindings", UNSET)
        bindings: list[BroadcastMediaLayerRequestBindingsItem] | Unset = UNSET
        if _bindings is not UNSET:
            bindings = []
            for bindings_item_data in _bindings:
                bindings_item = BroadcastMediaLayerRequestBindingsItem.from_dict(
                    bindings_item_data
                )

                bindings.append(bindings_item)

        blend = d.pop("blend", UNSET)

        enabled = d.pop("enabled", UNSET)

        def _parse_name(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        name = _parse_name(d.pop("name", UNSET))

        opacity = d.pop("opacity", UNSET)

        _playback = d.pop("playback", UNSET)
        playback: BroadcastMediaLayerRequestPlayback | Unset
        if isinstance(_playback, Unset):
            playback = UNSET
        else:
            playback = BroadcastMediaLayerRequestPlayback.from_dict(_playback)

        _targets = d.pop("targets", UNSET)
        targets: list[BroadcastMediaLayerRequestTargetsItem] | Unset = UNSET
        if _targets is not UNSET:
            targets = []
            for targets_item_data in _targets:
                targets_item = BroadcastMediaLayerRequestTargetsItem.from_dict(
                    targets_item_data
                )

                targets.append(targets_item)

        broadcast_media_layer_request = cls(
            asset_id=asset_id,
            bindings=bindings,
            blend=blend,
            enabled=enabled,
            name=name,
            opacity=opacity,
            playback=playback,
            targets=targets,
        )

        broadcast_media_layer_request.additional_properties = d
        return broadcast_media_layer_request

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
