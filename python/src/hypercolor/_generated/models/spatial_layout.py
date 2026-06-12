from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.edge_behavior import EdgeBehavior
    from ..models.output import Output
    from ..models.sampling_mode_type_0 import SamplingModeType0
    from ..models.sampling_mode_type_1 import SamplingModeType1
    from ..models.sampling_mode_type_2 import SamplingModeType2
    from ..models.sampling_mode_type_3 import SamplingModeType3
    from ..models.space_definition import SpaceDefinition


T = TypeVar("T", bound="SpatialLayout")


@_attrs_define
class SpatialLayout:
    """Top-level spatial layout container.

    Defines the complete mapping from a 2D effect canvas to the physical LED
    positions of every connected device. All coordinates use normalized
    `[0.0, 1.0]` space where `(0,0)` is top-left and `(1,1)` is bottom-right.

        Attributes:
            canvas_height (int): Canvas height in pixels. Standard: 200.
            canvas_width (int): Canvas width in pixels. Standard: 320.
            id (str): Unique layout identifier (UUID or slug).
            name (str): Human-readable name (e.g., "Bliss's PC Case", "Full Room").
            version (int): Schema version for forward-compatible migrations.
            zones (list[Output]): All device zones in this layout, ordered by rendering priority.
            default_edge_behavior (EdgeBehavior | Unset): Edge behavior for out-of-bounds LED positions.
            default_sampling_mode (SamplingModeType0 | SamplingModeType1 | SamplingModeType2 | SamplingModeType3 | Unset):
                Sampling algorithm for canvas-to-LED color extraction.
            description (None | str | Unset): Optional description for the layout editor UI.
            spaces (list[SpaceDefinition] | None | Unset): Space hierarchy for multi-room layouts.
                `None` means all zones live in a flat canvas (device/desk scale).
    """

    canvas_height: int
    canvas_width: int
    id: str
    name: str
    version: int
    zones: list[Output]
    default_edge_behavior: EdgeBehavior | Unset = UNSET
    default_sampling_mode: (
        SamplingModeType0
        | SamplingModeType1
        | SamplingModeType2
        | SamplingModeType3
        | Unset
    ) = UNSET
    description: None | str | Unset = UNSET
    spaces: list[SpaceDefinition] | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.sampling_mode_type_0 import SamplingModeType0
        from ..models.sampling_mode_type_1 import SamplingModeType1
        from ..models.sampling_mode_type_2 import SamplingModeType2

        canvas_height = self.canvas_height

        canvas_width = self.canvas_width

        id = self.id

        name = self.name

        version = self.version

        zones = []
        for zones_item_data in self.zones:
            zones_item = zones_item_data.to_dict()
            zones.append(zones_item)

        default_edge_behavior: dict[str, Any] | Unset = UNSET
        if not isinstance(self.default_edge_behavior, Unset):
            default_edge_behavior = self.default_edge_behavior.to_dict()

        default_sampling_mode: dict[str, Any] | Unset
        if isinstance(self.default_sampling_mode, Unset):
            default_sampling_mode = UNSET
        elif isinstance(self.default_sampling_mode, SamplingModeType0):
            default_sampling_mode = self.default_sampling_mode.to_dict()
        elif isinstance(self.default_sampling_mode, SamplingModeType1):
            default_sampling_mode = self.default_sampling_mode.to_dict()
        elif isinstance(self.default_sampling_mode, SamplingModeType2):
            default_sampling_mode = self.default_sampling_mode.to_dict()
        else:
            default_sampling_mode = self.default_sampling_mode.to_dict()

        description: None | str | Unset
        if isinstance(self.description, Unset):
            description = UNSET
        else:
            description = self.description

        spaces: list[dict[str, Any]] | None | Unset
        if isinstance(self.spaces, Unset):
            spaces = UNSET
        elif isinstance(self.spaces, list):
            spaces = []
            for spaces_type_0_item_data in self.spaces:
                spaces_type_0_item = spaces_type_0_item_data.to_dict()
                spaces.append(spaces_type_0_item)

        else:
            spaces = self.spaces

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "canvas_height": canvas_height,
                "canvas_width": canvas_width,
                "id": id,
                "name": name,
                "version": version,
                "zones": zones,
            }
        )
        if default_edge_behavior is not UNSET:
            field_dict["default_edge_behavior"] = default_edge_behavior
        if default_sampling_mode is not UNSET:
            field_dict["default_sampling_mode"] = default_sampling_mode
        if description is not UNSET:
            field_dict["description"] = description
        if spaces is not UNSET:
            field_dict["spaces"] = spaces

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.edge_behavior import EdgeBehavior
        from ..models.output import Output
        from ..models.sampling_mode_type_0 import SamplingModeType0
        from ..models.sampling_mode_type_1 import SamplingModeType1
        from ..models.sampling_mode_type_2 import SamplingModeType2
        from ..models.sampling_mode_type_3 import SamplingModeType3
        from ..models.space_definition import SpaceDefinition

        d = dict(src_dict)
        canvas_height = d.pop("canvas_height")

        canvas_width = d.pop("canvas_width")

        id = d.pop("id")

        name = d.pop("name")

        version = d.pop("version")

        zones = []
        _zones = d.pop("zones")
        for zones_item_data in _zones:
            zones_item = Output.from_dict(zones_item_data)

            zones.append(zones_item)

        _default_edge_behavior = d.pop("default_edge_behavior", UNSET)
        default_edge_behavior: EdgeBehavior | Unset
        if isinstance(_default_edge_behavior, Unset):
            default_edge_behavior = UNSET
        else:
            default_edge_behavior = EdgeBehavior.from_dict(_default_edge_behavior)

        def _parse_default_sampling_mode(
            data: object,
        ) -> (
            SamplingModeType0
            | SamplingModeType1
            | SamplingModeType2
            | SamplingModeType3
            | Unset
        ):
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_sampling_mode_type_0 = SamplingModeType0.from_dict(
                    data
                )

                return componentsschemas_sampling_mode_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_sampling_mode_type_1 = SamplingModeType1.from_dict(
                    data
                )

                return componentsschemas_sampling_mode_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_sampling_mode_type_2 = SamplingModeType2.from_dict(
                    data
                )

                return componentsschemas_sampling_mode_type_2
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            if not isinstance(data, dict):
                raise TypeError()
            componentsschemas_sampling_mode_type_3 = SamplingModeType3.from_dict(data)

            return componentsschemas_sampling_mode_type_3

        default_sampling_mode = _parse_default_sampling_mode(
            d.pop("default_sampling_mode", UNSET)
        )

        def _parse_description(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        description = _parse_description(d.pop("description", UNSET))

        def _parse_spaces(data: object) -> list[SpaceDefinition] | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, list):
                    raise TypeError()
                spaces_type_0 = []
                _spaces_type_0 = data
                for spaces_type_0_item_data in _spaces_type_0:
                    spaces_type_0_item = SpaceDefinition.from_dict(
                        spaces_type_0_item_data
                    )

                    spaces_type_0.append(spaces_type_0_item)

                return spaces_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(list[SpaceDefinition] | None | Unset, data)

        spaces = _parse_spaces(d.pop("spaces", UNSET))

        spatial_layout = cls(
            canvas_height=canvas_height,
            canvas_width=canvas_width,
            id=id,
            name=name,
            version=version,
            zones=zones,
            default_edge_behavior=default_edge_behavior,
            default_sampling_mode=default_sampling_mode,
            description=description,
            spaces=spaces,
        )

        spatial_layout.additional_properties = d
        return spatial_layout

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
