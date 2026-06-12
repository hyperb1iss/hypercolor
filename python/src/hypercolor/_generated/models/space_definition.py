from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.normalized_rect import NormalizedRect
    from ..models.room_adjacency import RoomAdjacency
    from ..models.room_dimensions import RoomDimensions


T = TypeVar("T", bound="SpaceDefinition")


@_attrs_define
class SpaceDefinition:
    """A physical space (room) containing a subset of zones.

    Used for multi-room orchestration and per-room canvas rendering.

        Attributes:
            adjacency (list[RoomAdjacency]): Neighboring spaces that share walls with this one.
            id (str): Unique space identifier.
            name (str): Human-readable name (e.g., "Office", "Living Room").
            zone_ids (list[str]): IDs of zones belonging to this space.
            canvas_region (None | NormalizedRect | Unset):
            dimensions (None | RoomDimensions | Unset):
    """

    adjacency: list[RoomAdjacency]
    id: str
    name: str
    zone_ids: list[str]
    canvas_region: None | NormalizedRect | Unset = UNSET
    dimensions: None | RoomDimensions | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.normalized_rect import NormalizedRect
        from ..models.room_dimensions import RoomDimensions

        adjacency = []
        for adjacency_item_data in self.adjacency:
            adjacency_item = adjacency_item_data.to_dict()
            adjacency.append(adjacency_item)

        id = self.id

        name = self.name

        zone_ids = self.zone_ids

        canvas_region: dict[str, Any] | None | Unset
        if isinstance(self.canvas_region, Unset):
            canvas_region = UNSET
        elif isinstance(self.canvas_region, NormalizedRect):
            canvas_region = self.canvas_region.to_dict()
        else:
            canvas_region = self.canvas_region

        dimensions: dict[str, Any] | None | Unset
        if isinstance(self.dimensions, Unset):
            dimensions = UNSET
        elif isinstance(self.dimensions, RoomDimensions):
            dimensions = self.dimensions.to_dict()
        else:
            dimensions = self.dimensions

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "adjacency": adjacency,
                "id": id,
                "name": name,
                "zone_ids": zone_ids,
            }
        )
        if canvas_region is not UNSET:
            field_dict["canvas_region"] = canvas_region
        if dimensions is not UNSET:
            field_dict["dimensions"] = dimensions

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.normalized_rect import NormalizedRect
        from ..models.room_adjacency import RoomAdjacency
        from ..models.room_dimensions import RoomDimensions

        d = dict(src_dict)
        adjacency = []
        _adjacency = d.pop("adjacency")
        for adjacency_item_data in _adjacency:
            adjacency_item = RoomAdjacency.from_dict(adjacency_item_data)

            adjacency.append(adjacency_item)

        id = d.pop("id")

        name = d.pop("name")

        zone_ids = cast(list[str], d.pop("zone_ids"))

        def _parse_canvas_region(data: object) -> None | NormalizedRect | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                canvas_region_type_1 = NormalizedRect.from_dict(data)

                return canvas_region_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | NormalizedRect | Unset, data)

        canvas_region = _parse_canvas_region(d.pop("canvas_region", UNSET))

        def _parse_dimensions(data: object) -> None | RoomDimensions | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                dimensions_type_1 = RoomDimensions.from_dict(data)

                return dimensions_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | RoomDimensions | Unset, data)

        dimensions = _parse_dimensions(d.pop("dimensions", UNSET))

        space_definition = cls(
            adjacency=adjacency,
            id=id,
            name=name,
            zone_ids=zone_ids,
            canvas_region=canvas_region,
            dimensions=dimensions,
        )

        space_definition.additional_properties = d
        return space_definition

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
