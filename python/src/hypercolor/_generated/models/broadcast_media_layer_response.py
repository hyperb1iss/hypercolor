from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.broadcast_media_layer_zone_response import (
        BroadcastMediaLayerZoneResponse,
    )


T = TypeVar("T", bound="BroadcastMediaLayerResponse")


@_attrs_define
class BroadcastMediaLayerResponse:
    """
    Attributes:
        zones (list[BroadcastMediaLayerZoneResponse]):
    """

    zones: list[BroadcastMediaLayerZoneResponse]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        zones = []
        for zones_item_data in self.zones:
            zones_item = zones_item_data.to_dict()
            zones.append(zones_item)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "zones": zones,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.broadcast_media_layer_zone_response import (
            BroadcastMediaLayerZoneResponse,
        )

        d = dict(src_dict)
        zones = []
        _zones = d.pop("zones")
        for zones_item_data in _zones:
            zones_item = BroadcastMediaLayerZoneResponse.from_dict(zones_item_data)

            zones.append(zones_item)

        broadcast_media_layer_response = cls(
            zones=zones,
        )

        broadcast_media_layer_response.additional_properties = d
        return broadcast_media_layer_response

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
