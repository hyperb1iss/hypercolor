from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from typing_extensions import Self

T = TypeVar("T", bound="LayerSlot")


@_attrs_define
class LayerSlot:
    """A layer's place in a scene: the zone that owns it and its position in
    that zone's bottom-to-top stack.

        Attributes:
            index (int):
            zone_id (str):
    """

    index: int
    zone_id: str

    def to_dict(self) -> dict[str, Any]:
        index = self.index

        zone_id = self.zone_id

        field_dict: dict[str, Any] = {}

        field_dict.update(
            {
                "index": index,
                "zone_id": zone_id,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        index = d.pop("index")

        zone_id = d.pop("zone_id")

        layer_slot = cls(
            index=index,
            zone_id=zone_id,
        )

        return layer_slot
