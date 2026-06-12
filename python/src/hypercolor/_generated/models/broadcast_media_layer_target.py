from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.broadcast_media_layer_target_adjust import (
        BroadcastMediaLayerTargetAdjust,
    )
    from ..models.broadcast_media_layer_target_transform import (
        BroadcastMediaLayerTargetTransform,
    )


T = TypeVar("T", bound="BroadcastMediaLayerTarget")


@_attrs_define
class BroadcastMediaLayerTarget:
    """
    Attributes:
        group_id (str):
        adjust (BroadcastMediaLayerTargetAdjust | Unset):
        expected_layers_version (int | None | Unset):
        index (int | None | Unset):
        transform (BroadcastMediaLayerTargetTransform | Unset):
    """

    group_id: str
    adjust: BroadcastMediaLayerTargetAdjust | Unset = UNSET
    expected_layers_version: int | None | Unset = UNSET
    index: int | None | Unset = UNSET
    transform: BroadcastMediaLayerTargetTransform | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        group_id = self.group_id

        adjust: dict[str, Any] | Unset = UNSET
        if not isinstance(self.adjust, Unset):
            adjust = self.adjust.to_dict()

        expected_layers_version: int | None | Unset
        if isinstance(self.expected_layers_version, Unset):
            expected_layers_version = UNSET
        else:
            expected_layers_version = self.expected_layers_version

        index: int | None | Unset
        if isinstance(self.index, Unset):
            index = UNSET
        else:
            index = self.index

        transform: dict[str, Any] | Unset = UNSET
        if not isinstance(self.transform, Unset):
            transform = self.transform.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "group_id": group_id,
            }
        )
        if adjust is not UNSET:
            field_dict["adjust"] = adjust
        if expected_layers_version is not UNSET:
            field_dict["expected_layers_version"] = expected_layers_version
        if index is not UNSET:
            field_dict["index"] = index
        if transform is not UNSET:
            field_dict["transform"] = transform

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.broadcast_media_layer_target_adjust import (
            BroadcastMediaLayerTargetAdjust,
        )
        from ..models.broadcast_media_layer_target_transform import (
            BroadcastMediaLayerTargetTransform,
        )

        d = dict(src_dict)
        group_id = d.pop("group_id")

        _adjust = d.pop("adjust", UNSET)
        adjust: BroadcastMediaLayerTargetAdjust | Unset
        if isinstance(_adjust, Unset):
            adjust = UNSET
        else:
            adjust = BroadcastMediaLayerTargetAdjust.from_dict(_adjust)

        def _parse_expected_layers_version(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        expected_layers_version = _parse_expected_layers_version(
            d.pop("expected_layers_version", UNSET)
        )

        def _parse_index(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        index = _parse_index(d.pop("index", UNSET))

        _transform = d.pop("transform", UNSET)
        transform: BroadcastMediaLayerTargetTransform | Unset
        if isinstance(_transform, Unset):
            transform = UNSET
        else:
            transform = BroadcastMediaLayerTargetTransform.from_dict(_transform)

        broadcast_media_layer_target = cls(
            group_id=group_id,
            adjust=adjust,
            expected_layers_version=expected_layers_version,
            index=index,
            transform=transform,
        )

        broadcast_media_layer_target.additional_properties = d
        return broadcast_media_layer_target

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
