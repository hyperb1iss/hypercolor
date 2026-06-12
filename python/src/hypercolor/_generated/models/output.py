from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.orientation import Orientation
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.edge_behavior import EdgeBehavior
    from ..models.led_topology_type_0 import LedTopologyType0
    from ..models.led_topology_type_1 import LedTopologyType1
    from ..models.led_topology_type_2 import LedTopologyType2
    from ..models.led_topology_type_3 import LedTopologyType3
    from ..models.led_topology_type_4 import LedTopologyType4
    from ..models.led_topology_type_5 import LedTopologyType5
    from ..models.led_topology_type_6 import LedTopologyType6
    from ..models.normalized_position import NormalizedPosition
    from ..models.output_component import OutputComponent
    from ..models.sampling_mode_type_0 import SamplingModeType0
    from ..models.sampling_mode_type_1 import SamplingModeType1
    from ..models.sampling_mode_type_2 import SamplingModeType2
    from ..models.sampling_mode_type_3 import SamplingModeType3
    from ..models.zone_shape_type_0 import ZoneShapeType0
    from ..models.zone_shape_type_1 import ZoneShapeType1
    from ..models.zone_shape_type_2 import ZoneShapeType2
    from ..models.zone_shape_type_3 import ZoneShapeType3


T = TypeVar("T", bound="Output")


@_attrs_define
class Output:
    """A device zone: the spatial binding between a physical device and a
    region of the effect canvas.

    The zone's bounding rectangle is defined by `position` (center) and
    `size` (width, height), both in normalized `[0.0, 1.0]` canvas coordinates.
    LED positions within the zone are computed from the `topology` and stored
    in `led_positions` as zone-local normalized coordinates.

        Attributes:
            device_id (str): Backend device identifier.
                Format: `"<backend>:<device_id>"` (e.g., `"usb:controller-1"`, `"network:node-42"`).
            id (str): Unique identifier within the layout.
            name (str): Human-readable name (e.g., "ATX Strimer", "Front Fan 1").
            position (NormalizedPosition): A position in normalized `[0.0, 1.0]` canvas space.

                - `(0.0, 0.0)` = top-left corner of the canvas
                - `(1.0, 1.0)` = bottom-right corner of the canvas
                - `(0.5, 0.5)` = center of the canvas

                Values outside `[0.0, 1.0]` are permitted — they represent positions
                beyond the canvas bounds and are handled by [`EdgeBehavior`].

                Used for zone positions and sizes on the canvas, LED positions within
                a zone's bounding box, and space regions in multi-room layouts.
            rotation (float): Rotation in radians around the zone's center point.
                Positive = counter-clockwise (standard math convention).
            size (NormalizedPosition): A position in normalized `[0.0, 1.0]` canvas space.

                - `(0.0, 0.0)` = top-left corner of the canvas
                - `(1.0, 1.0)` = bottom-right corner of the canvas
                - `(0.5, 0.5)` = center of the canvas

                Values outside `[0.0, 1.0]` are permitted — they represent positions
                beyond the canvas bounds and are handled by [`EdgeBehavior`].

                Used for zone positions and sizes on the canvas, LED positions within
                a zone's bounding box, and space regions in multi-room layouts.
            topology (LedTopologyType0 | LedTopologyType1 | LedTopologyType2 | LedTopologyType3 | LedTopologyType4 |
                LedTopologyType5 | LedTopologyType6): LED arrangement within a zone's bounding rectangle.

                Each variant computes zone-local positions in normalized `[0.0, 1.0]` space.
                The topology determines how many LEDs exist and where they sit within
                the zone's rectangular bounds.
            attachment (None | OutputComponent | Unset):
            brightness (float | None | Unset): Per-zone brightness scalar in `[0.0, 1.0]`. `None` means full
                brightness (1.0). Applied multiplicatively with the device output
                brightness and global scene brightness during frame routing.

                Use this to dim a single channel of a multi-zone controller
                without touching its siblings — e.g. balancing a specific LED
                strip against the rest of the setup.
            display_order (int | Unset): Display stacking order in the layout editor.
                Higher values render on top. Zones with equal values use vector order.
            edge_behavior (EdgeBehavior | None | Unset):
            led_mapping (list[int] | None | Unset): Optional spatial-index -> physical-index remap applied before device
                writes.

                Attachment templates use this to preserve non-sequential wiring orders
                without baking transport details into topology coordinates.
            orientation (None | Orientation | Unset):
            sampling_mode (None | SamplingModeType0 | SamplingModeType1 | SamplingModeType2 | SamplingModeType3 | Unset):
            scale (float | Unset): Scale factor applied uniformly. Default 1.0.
            shape (None | Unset | ZoneShapeType0 | ZoneShapeType1 | ZoneShapeType2 | ZoneShapeType3):
            shape_preset (None | str | Unset): Shape preset ID from the device library (e.g., `"strimer-atx-24pin"`).
            zone_name (None | str | Unset): Sub-device channel or segment name (e.g., `"ch1"`, `"atx"`, `"segment-0"`).
                `None` for single-zone devices.
    """

    device_id: str
    id: str
    name: str
    position: NormalizedPosition
    rotation: float
    size: NormalizedPosition
    topology: (
        LedTopologyType0
        | LedTopologyType1
        | LedTopologyType2
        | LedTopologyType3
        | LedTopologyType4
        | LedTopologyType5
        | LedTopologyType6
    )
    attachment: None | OutputComponent | Unset = UNSET
    brightness: float | None | Unset = UNSET
    display_order: int | Unset = UNSET
    edge_behavior: EdgeBehavior | None | Unset = UNSET
    led_mapping: list[int] | None | Unset = UNSET
    orientation: None | Orientation | Unset = UNSET
    sampling_mode: (
        None
        | SamplingModeType0
        | SamplingModeType1
        | SamplingModeType2
        | SamplingModeType3
        | Unset
    ) = UNSET
    scale: float | Unset = UNSET
    shape: (
        None | Unset | ZoneShapeType0 | ZoneShapeType1 | ZoneShapeType2 | ZoneShapeType3
    ) = UNSET
    shape_preset: None | str | Unset = UNSET
    zone_name: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.edge_behavior import EdgeBehavior
        from ..models.led_topology_type_0 import LedTopologyType0
        from ..models.led_topology_type_1 import LedTopologyType1
        from ..models.led_topology_type_2 import LedTopologyType2
        from ..models.led_topology_type_3 import LedTopologyType3
        from ..models.led_topology_type_4 import LedTopologyType4
        from ..models.led_topology_type_5 import LedTopologyType5
        from ..models.output_component import OutputComponent
        from ..models.sampling_mode_type_0 import SamplingModeType0
        from ..models.sampling_mode_type_1 import SamplingModeType1
        from ..models.sampling_mode_type_2 import SamplingModeType2
        from ..models.sampling_mode_type_3 import SamplingModeType3
        from ..models.zone_shape_type_0 import ZoneShapeType0
        from ..models.zone_shape_type_1 import ZoneShapeType1
        from ..models.zone_shape_type_2 import ZoneShapeType2
        from ..models.zone_shape_type_3 import ZoneShapeType3

        device_id = self.device_id

        id = self.id

        name = self.name

        position = self.position.to_dict()

        rotation = self.rotation

        size = self.size.to_dict()

        topology: dict[str, Any]
        if isinstance(self.topology, LedTopologyType0):
            topology = self.topology.to_dict()
        elif isinstance(self.topology, LedTopologyType1):
            topology = self.topology.to_dict()
        elif isinstance(self.topology, LedTopologyType2):
            topology = self.topology.to_dict()
        elif isinstance(self.topology, LedTopologyType3):
            topology = self.topology.to_dict()
        elif isinstance(self.topology, LedTopologyType4):
            topology = self.topology.to_dict()
        elif isinstance(self.topology, LedTopologyType5):
            topology = self.topology.to_dict()
        else:
            topology = self.topology.to_dict()

        attachment: dict[str, Any] | None | Unset
        if isinstance(self.attachment, Unset):
            attachment = UNSET
        elif isinstance(self.attachment, OutputComponent):
            attachment = self.attachment.to_dict()
        else:
            attachment = self.attachment

        brightness: float | None | Unset
        if isinstance(self.brightness, Unset):
            brightness = UNSET
        else:
            brightness = self.brightness

        display_order = self.display_order

        edge_behavior: dict[str, Any] | None | Unset
        if isinstance(self.edge_behavior, Unset):
            edge_behavior = UNSET
        elif isinstance(self.edge_behavior, EdgeBehavior):
            edge_behavior = self.edge_behavior.to_dict()
        else:
            edge_behavior = self.edge_behavior

        led_mapping: list[int] | None | Unset
        if isinstance(self.led_mapping, Unset):
            led_mapping = UNSET
        elif isinstance(self.led_mapping, list):
            led_mapping = self.led_mapping

        else:
            led_mapping = self.led_mapping

        orientation: None | str | Unset
        if isinstance(self.orientation, Unset):
            orientation = UNSET
        elif isinstance(self.orientation, Orientation):
            orientation = self.orientation.value
        else:
            orientation = self.orientation

        sampling_mode: dict[str, Any] | None | Unset
        if isinstance(self.sampling_mode, Unset):
            sampling_mode = UNSET
        elif isinstance(self.sampling_mode, SamplingModeType0):
            sampling_mode = self.sampling_mode.to_dict()
        elif isinstance(self.sampling_mode, SamplingModeType1):
            sampling_mode = self.sampling_mode.to_dict()
        elif isinstance(self.sampling_mode, SamplingModeType2):
            sampling_mode = self.sampling_mode.to_dict()
        elif isinstance(self.sampling_mode, SamplingModeType3):
            sampling_mode = self.sampling_mode.to_dict()
        else:
            sampling_mode = self.sampling_mode

        scale = self.scale

        shape: dict[str, Any] | None | Unset
        if isinstance(self.shape, Unset):
            shape = UNSET
        elif isinstance(self.shape, ZoneShapeType0):
            shape = self.shape.to_dict()
        elif isinstance(self.shape, ZoneShapeType1):
            shape = self.shape.to_dict()
        elif isinstance(self.shape, ZoneShapeType2):
            shape = self.shape.to_dict()
        elif isinstance(self.shape, ZoneShapeType3):
            shape = self.shape.to_dict()
        else:
            shape = self.shape

        shape_preset: None | str | Unset
        if isinstance(self.shape_preset, Unset):
            shape_preset = UNSET
        else:
            shape_preset = self.shape_preset

        zone_name: None | str | Unset
        if isinstance(self.zone_name, Unset):
            zone_name = UNSET
        else:
            zone_name = self.zone_name

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "device_id": device_id,
                "id": id,
                "name": name,
                "position": position,
                "rotation": rotation,
                "size": size,
                "topology": topology,
            }
        )
        if attachment is not UNSET:
            field_dict["attachment"] = attachment
        if brightness is not UNSET:
            field_dict["brightness"] = brightness
        if display_order is not UNSET:
            field_dict["display_order"] = display_order
        if edge_behavior is not UNSET:
            field_dict["edge_behavior"] = edge_behavior
        if led_mapping is not UNSET:
            field_dict["led_mapping"] = led_mapping
        if orientation is not UNSET:
            field_dict["orientation"] = orientation
        if sampling_mode is not UNSET:
            field_dict["sampling_mode"] = sampling_mode
        if scale is not UNSET:
            field_dict["scale"] = scale
        if shape is not UNSET:
            field_dict["shape"] = shape
        if shape_preset is not UNSET:
            field_dict["shape_preset"] = shape_preset
        if zone_name is not UNSET:
            field_dict["zone_name"] = zone_name

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.edge_behavior import EdgeBehavior
        from ..models.led_topology_type_0 import LedTopologyType0
        from ..models.led_topology_type_1 import LedTopologyType1
        from ..models.led_topology_type_2 import LedTopologyType2
        from ..models.led_topology_type_3 import LedTopologyType3
        from ..models.led_topology_type_4 import LedTopologyType4
        from ..models.led_topology_type_5 import LedTopologyType5
        from ..models.led_topology_type_6 import LedTopologyType6
        from ..models.normalized_position import NormalizedPosition
        from ..models.output_component import OutputComponent
        from ..models.sampling_mode_type_0 import SamplingModeType0
        from ..models.sampling_mode_type_1 import SamplingModeType1
        from ..models.sampling_mode_type_2 import SamplingModeType2
        from ..models.sampling_mode_type_3 import SamplingModeType3
        from ..models.zone_shape_type_0 import ZoneShapeType0
        from ..models.zone_shape_type_1 import ZoneShapeType1
        from ..models.zone_shape_type_2 import ZoneShapeType2
        from ..models.zone_shape_type_3 import ZoneShapeType3

        d = dict(src_dict)
        device_id = d.pop("device_id")

        id = d.pop("id")

        name = d.pop("name")

        position = NormalizedPosition.from_dict(d.pop("position"))

        rotation = d.pop("rotation")

        size = NormalizedPosition.from_dict(d.pop("size"))

        def _parse_topology(
            data: object,
        ) -> (
            LedTopologyType0
            | LedTopologyType1
            | LedTopologyType2
            | LedTopologyType3
            | LedTopologyType4
            | LedTopologyType5
            | LedTopologyType6
        ):
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_led_topology_type_0 = LedTopologyType0.from_dict(data)

                return componentsschemas_led_topology_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_led_topology_type_1 = LedTopologyType1.from_dict(data)

                return componentsschemas_led_topology_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_led_topology_type_2 = LedTopologyType2.from_dict(data)

                return componentsschemas_led_topology_type_2
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_led_topology_type_3 = LedTopologyType3.from_dict(data)

                return componentsschemas_led_topology_type_3
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_led_topology_type_4 = LedTopologyType4.from_dict(data)

                return componentsschemas_led_topology_type_4
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_led_topology_type_5 = LedTopologyType5.from_dict(data)

                return componentsschemas_led_topology_type_5
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            if not isinstance(data, dict):
                raise TypeError()
            componentsschemas_led_topology_type_6 = LedTopologyType6.from_dict(data)

            return componentsschemas_led_topology_type_6

        topology = _parse_topology(d.pop("topology"))

        def _parse_attachment(data: object) -> None | OutputComponent | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                attachment_type_1 = OutputComponent.from_dict(data)

                return attachment_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | OutputComponent | Unset, data)

        attachment = _parse_attachment(d.pop("attachment", UNSET))

        def _parse_brightness(data: object) -> float | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(float | None | Unset, data)

        brightness = _parse_brightness(d.pop("brightness", UNSET))

        display_order = d.pop("display_order", UNSET)

        def _parse_edge_behavior(data: object) -> EdgeBehavior | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                edge_behavior_type_1 = EdgeBehavior.from_dict(data)

                return edge_behavior_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(EdgeBehavior | None | Unset, data)

        edge_behavior = _parse_edge_behavior(d.pop("edge_behavior", UNSET))

        def _parse_led_mapping(data: object) -> list[int] | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, list):
                    raise TypeError()
                led_mapping_type_0 = cast(list[int], data)

                return led_mapping_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(list[int] | None | Unset, data)

        led_mapping = _parse_led_mapping(d.pop("led_mapping", UNSET))

        def _parse_orientation(data: object) -> None | Orientation | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                orientation_type_1 = Orientation(data)

                return orientation_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | Orientation | Unset, data)

        orientation = _parse_orientation(d.pop("orientation", UNSET))

        def _parse_sampling_mode(
            data: object,
        ) -> (
            None
            | SamplingModeType0
            | SamplingModeType1
            | SamplingModeType2
            | SamplingModeType3
            | Unset
        ):
            if data is None:
                return data
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
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_sampling_mode_type_3 = SamplingModeType3.from_dict(
                    data
                )

                return componentsschemas_sampling_mode_type_3
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(
                None
                | SamplingModeType0
                | SamplingModeType1
                | SamplingModeType2
                | SamplingModeType3
                | Unset,
                data,
            )

        sampling_mode = _parse_sampling_mode(d.pop("sampling_mode", UNSET))

        scale = d.pop("scale", UNSET)

        def _parse_shape(
            data: object,
        ) -> (
            None
            | Unset
            | ZoneShapeType0
            | ZoneShapeType1
            | ZoneShapeType2
            | ZoneShapeType3
        ):
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_zone_shape_type_0 = ZoneShapeType0.from_dict(data)

                return componentsschemas_zone_shape_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_zone_shape_type_1 = ZoneShapeType1.from_dict(data)

                return componentsschemas_zone_shape_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_zone_shape_type_2 = ZoneShapeType2.from_dict(data)

                return componentsschemas_zone_shape_type_2
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_zone_shape_type_3 = ZoneShapeType3.from_dict(data)

                return componentsschemas_zone_shape_type_3
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(
                None
                | Unset
                | ZoneShapeType0
                | ZoneShapeType1
                | ZoneShapeType2
                | ZoneShapeType3,
                data,
            )

        shape = _parse_shape(d.pop("shape", UNSET))

        def _parse_shape_preset(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        shape_preset = _parse_shape_preset(d.pop("shape_preset", UNSET))

        def _parse_zone_name(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        zone_name = _parse_zone_name(d.pop("zone_name", UNSET))

        output = cls(
            device_id=device_id,
            id=id,
            name=name,
            position=position,
            rotation=rotation,
            size=size,
            topology=topology,
            attachment=attachment,
            brightness=brightness,
            display_order=display_order,
            edge_behavior=edge_behavior,
            led_mapping=led_mapping,
            orientation=orientation,
            sampling_mode=sampling_mode,
            scale=scale,
            shape=shape,
            shape_preset=shape_preset,
            zone_name=zone_name,
        )

        output.additional_properties = d
        return output

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
