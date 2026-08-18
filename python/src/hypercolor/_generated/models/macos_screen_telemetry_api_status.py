from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.macos_architecture_api import MacosArchitectureApi
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.macos_frame_drop_api_status import MacosFrameDropApiStatus
    from ..models.macos_screen_timing_api_status import MacosScreenTimingApiStatus


T = TypeVar("T", bound="MacosScreenTelemetryApiStatus")


@_attrs_define
class MacosScreenTelemetryApiStatus:
    """
    Attributes:
        admitted_native_bytes (int):
        callback_max_ns (int):
        callback_total_ns (int):
        conversion_max_ns (int):
        conversion_total_ns (int):
        cpu_reduction_max_ns (int):
        cpu_reduction_total_ns (int):
        executable_architecture (MacosArchitectureApi):
        frames_dropped (list[MacosFrameDropApiStatus]):
        frames_malformed (int):
        frames_published (int):
        frames_received (int):
        frames_stale (int):
        frames_superseded (int):
        native_import_max_ns (int):
        native_import_total_ns (int):
        native_reduction_submit_max_ns (int):
        native_reduction_submit_total_ns (int):
        publication_max_ns (int):
        publication_total_ns (int):
        queue_depth (int):
        retain_max_ns (int):
        retain_total_ns (int):
        stream_state (str):
        authorization_last_transition_age_ms (int | None | Unset):
        capture_session_generation (int | None | Unset):
        color_space (None | str | Unset):
        display_scale (float | None | Unset):
        dynamic_range (None | str | Unset):
        fallback_reason (None | str | Unset):
        native_height (int | None | Unset):
        native_width (int | None | Unset):
        owner_designated_requirement_hash (None | str | Unset):
        pinned_generations (int | None | Unset):
        pixel_format (None | str | Unset):
        publication_path (None | str | Unset):
        publication_plan_generation (int | None | Unset):
        resource_generation (int | None | Unset):
        selection_diagnostic_label (None | str | Unset):
        timing (MacosScreenTimingApiStatus | None | Unset):
        topology_generation (int | None | Unset):
        transfer_function (None | str | Unset):
    """

    admitted_native_bytes: int
    callback_max_ns: int
    callback_total_ns: int
    conversion_max_ns: int
    conversion_total_ns: int
    cpu_reduction_max_ns: int
    cpu_reduction_total_ns: int
    executable_architecture: MacosArchitectureApi
    frames_dropped: list[MacosFrameDropApiStatus]
    frames_malformed: int
    frames_published: int
    frames_received: int
    frames_stale: int
    frames_superseded: int
    native_import_max_ns: int
    native_import_total_ns: int
    native_reduction_submit_max_ns: int
    native_reduction_submit_total_ns: int
    publication_max_ns: int
    publication_total_ns: int
    queue_depth: int
    retain_max_ns: int
    retain_total_ns: int
    stream_state: str
    authorization_last_transition_age_ms: int | None | Unset = UNSET
    capture_session_generation: int | None | Unset = UNSET
    color_space: None | str | Unset = UNSET
    display_scale: float | None | Unset = UNSET
    dynamic_range: None | str | Unset = UNSET
    fallback_reason: None | str | Unset = UNSET
    native_height: int | None | Unset = UNSET
    native_width: int | None | Unset = UNSET
    owner_designated_requirement_hash: None | str | Unset = UNSET
    pinned_generations: int | None | Unset = UNSET
    pixel_format: None | str | Unset = UNSET
    publication_path: None | str | Unset = UNSET
    publication_plan_generation: int | None | Unset = UNSET
    resource_generation: int | None | Unset = UNSET
    selection_diagnostic_label: None | str | Unset = UNSET
    timing: MacosScreenTimingApiStatus | None | Unset = UNSET
    topology_generation: int | None | Unset = UNSET
    transfer_function: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.macos_screen_timing_api_status import MacosScreenTimingApiStatus

        admitted_native_bytes = self.admitted_native_bytes

        callback_max_ns = self.callback_max_ns

        callback_total_ns = self.callback_total_ns

        conversion_max_ns = self.conversion_max_ns

        conversion_total_ns = self.conversion_total_ns

        cpu_reduction_max_ns = self.cpu_reduction_max_ns

        cpu_reduction_total_ns = self.cpu_reduction_total_ns

        executable_architecture = self.executable_architecture.value

        frames_dropped = []
        for frames_dropped_item_data in self.frames_dropped:
            frames_dropped_item = frames_dropped_item_data.to_dict()
            frames_dropped.append(frames_dropped_item)

        frames_malformed = self.frames_malformed

        frames_published = self.frames_published

        frames_received = self.frames_received

        frames_stale = self.frames_stale

        frames_superseded = self.frames_superseded

        native_import_max_ns = self.native_import_max_ns

        native_import_total_ns = self.native_import_total_ns

        native_reduction_submit_max_ns = self.native_reduction_submit_max_ns

        native_reduction_submit_total_ns = self.native_reduction_submit_total_ns

        publication_max_ns = self.publication_max_ns

        publication_total_ns = self.publication_total_ns

        queue_depth = self.queue_depth

        retain_max_ns = self.retain_max_ns

        retain_total_ns = self.retain_total_ns

        stream_state = self.stream_state

        authorization_last_transition_age_ms: int | None | Unset
        if isinstance(self.authorization_last_transition_age_ms, Unset):
            authorization_last_transition_age_ms = UNSET
        else:
            authorization_last_transition_age_ms = (
                self.authorization_last_transition_age_ms
            )

        capture_session_generation: int | None | Unset
        if isinstance(self.capture_session_generation, Unset):
            capture_session_generation = UNSET
        else:
            capture_session_generation = self.capture_session_generation

        color_space: None | str | Unset
        if isinstance(self.color_space, Unset):
            color_space = UNSET
        else:
            color_space = self.color_space

        display_scale: float | None | Unset
        if isinstance(self.display_scale, Unset):
            display_scale = UNSET
        else:
            display_scale = self.display_scale

        dynamic_range: None | str | Unset
        if isinstance(self.dynamic_range, Unset):
            dynamic_range = UNSET
        else:
            dynamic_range = self.dynamic_range

        fallback_reason: None | str | Unset
        if isinstance(self.fallback_reason, Unset):
            fallback_reason = UNSET
        else:
            fallback_reason = self.fallback_reason

        native_height: int | None | Unset
        if isinstance(self.native_height, Unset):
            native_height = UNSET
        else:
            native_height = self.native_height

        native_width: int | None | Unset
        if isinstance(self.native_width, Unset):
            native_width = UNSET
        else:
            native_width = self.native_width

        owner_designated_requirement_hash: None | str | Unset
        if isinstance(self.owner_designated_requirement_hash, Unset):
            owner_designated_requirement_hash = UNSET
        else:
            owner_designated_requirement_hash = self.owner_designated_requirement_hash

        pinned_generations: int | None | Unset
        if isinstance(self.pinned_generations, Unset):
            pinned_generations = UNSET
        else:
            pinned_generations = self.pinned_generations

        pixel_format: None | str | Unset
        if isinstance(self.pixel_format, Unset):
            pixel_format = UNSET
        else:
            pixel_format = self.pixel_format

        publication_path: None | str | Unset
        if isinstance(self.publication_path, Unset):
            publication_path = UNSET
        else:
            publication_path = self.publication_path

        publication_plan_generation: int | None | Unset
        if isinstance(self.publication_plan_generation, Unset):
            publication_plan_generation = UNSET
        else:
            publication_plan_generation = self.publication_plan_generation

        resource_generation: int | None | Unset
        if isinstance(self.resource_generation, Unset):
            resource_generation = UNSET
        else:
            resource_generation = self.resource_generation

        selection_diagnostic_label: None | str | Unset
        if isinstance(self.selection_diagnostic_label, Unset):
            selection_diagnostic_label = UNSET
        else:
            selection_diagnostic_label = self.selection_diagnostic_label

        timing: dict[str, Any] | None | Unset
        if isinstance(self.timing, Unset):
            timing = UNSET
        elif isinstance(self.timing, MacosScreenTimingApiStatus):
            timing = self.timing.to_dict()
        else:
            timing = self.timing

        topology_generation: int | None | Unset
        if isinstance(self.topology_generation, Unset):
            topology_generation = UNSET
        else:
            topology_generation = self.topology_generation

        transfer_function: None | str | Unset
        if isinstance(self.transfer_function, Unset):
            transfer_function = UNSET
        else:
            transfer_function = self.transfer_function

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "admitted_native_bytes": admitted_native_bytes,
                "callback_max_ns": callback_max_ns,
                "callback_total_ns": callback_total_ns,
                "conversion_max_ns": conversion_max_ns,
                "conversion_total_ns": conversion_total_ns,
                "cpu_reduction_max_ns": cpu_reduction_max_ns,
                "cpu_reduction_total_ns": cpu_reduction_total_ns,
                "executable_architecture": executable_architecture,
                "frames_dropped": frames_dropped,
                "frames_malformed": frames_malformed,
                "frames_published": frames_published,
                "frames_received": frames_received,
                "frames_stale": frames_stale,
                "frames_superseded": frames_superseded,
                "native_import_max_ns": native_import_max_ns,
                "native_import_total_ns": native_import_total_ns,
                "native_reduction_submit_max_ns": native_reduction_submit_max_ns,
                "native_reduction_submit_total_ns": native_reduction_submit_total_ns,
                "publication_max_ns": publication_max_ns,
                "publication_total_ns": publication_total_ns,
                "queue_depth": queue_depth,
                "retain_max_ns": retain_max_ns,
                "retain_total_ns": retain_total_ns,
                "stream_state": stream_state,
            }
        )
        if authorization_last_transition_age_ms is not UNSET:
            field_dict["authorization_last_transition_age_ms"] = (
                authorization_last_transition_age_ms
            )
        if capture_session_generation is not UNSET:
            field_dict["capture_session_generation"] = capture_session_generation
        if color_space is not UNSET:
            field_dict["color_space"] = color_space
        if display_scale is not UNSET:
            field_dict["display_scale"] = display_scale
        if dynamic_range is not UNSET:
            field_dict["dynamic_range"] = dynamic_range
        if fallback_reason is not UNSET:
            field_dict["fallback_reason"] = fallback_reason
        if native_height is not UNSET:
            field_dict["native_height"] = native_height
        if native_width is not UNSET:
            field_dict["native_width"] = native_width
        if owner_designated_requirement_hash is not UNSET:
            field_dict["owner_designated_requirement_hash"] = (
                owner_designated_requirement_hash
            )
        if pinned_generations is not UNSET:
            field_dict["pinned_generations"] = pinned_generations
        if pixel_format is not UNSET:
            field_dict["pixel_format"] = pixel_format
        if publication_path is not UNSET:
            field_dict["publication_path"] = publication_path
        if publication_plan_generation is not UNSET:
            field_dict["publication_plan_generation"] = publication_plan_generation
        if resource_generation is not UNSET:
            field_dict["resource_generation"] = resource_generation
        if selection_diagnostic_label is not UNSET:
            field_dict["selection_diagnostic_label"] = selection_diagnostic_label
        if timing is not UNSET:
            field_dict["timing"] = timing
        if topology_generation is not UNSET:
            field_dict["topology_generation"] = topology_generation
        if transfer_function is not UNSET:
            field_dict["transfer_function"] = transfer_function

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.macos_frame_drop_api_status import MacosFrameDropApiStatus
        from ..models.macos_screen_timing_api_status import MacosScreenTimingApiStatus

        d = dict(src_dict)
        admitted_native_bytes = d.pop("admitted_native_bytes")

        callback_max_ns = d.pop("callback_max_ns")

        callback_total_ns = d.pop("callback_total_ns")

        conversion_max_ns = d.pop("conversion_max_ns")

        conversion_total_ns = d.pop("conversion_total_ns")

        cpu_reduction_max_ns = d.pop("cpu_reduction_max_ns")

        cpu_reduction_total_ns = d.pop("cpu_reduction_total_ns")

        executable_architecture = MacosArchitectureApi(d.pop("executable_architecture"))

        frames_dropped = []
        _frames_dropped = d.pop("frames_dropped")
        for frames_dropped_item_data in _frames_dropped:
            frames_dropped_item = MacosFrameDropApiStatus.from_dict(
                frames_dropped_item_data
            )

            frames_dropped.append(frames_dropped_item)

        frames_malformed = d.pop("frames_malformed")

        frames_published = d.pop("frames_published")

        frames_received = d.pop("frames_received")

        frames_stale = d.pop("frames_stale")

        frames_superseded = d.pop("frames_superseded")

        native_import_max_ns = d.pop("native_import_max_ns")

        native_import_total_ns = d.pop("native_import_total_ns")

        native_reduction_submit_max_ns = d.pop("native_reduction_submit_max_ns")

        native_reduction_submit_total_ns = d.pop("native_reduction_submit_total_ns")

        publication_max_ns = d.pop("publication_max_ns")

        publication_total_ns = d.pop("publication_total_ns")

        queue_depth = d.pop("queue_depth")

        retain_max_ns = d.pop("retain_max_ns")

        retain_total_ns = d.pop("retain_total_ns")

        stream_state = d.pop("stream_state")

        def _parse_authorization_last_transition_age_ms(
            data: object,
        ) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        authorization_last_transition_age_ms = (
            _parse_authorization_last_transition_age_ms(
                d.pop("authorization_last_transition_age_ms", UNSET)
            )
        )

        def _parse_capture_session_generation(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        capture_session_generation = _parse_capture_session_generation(
            d.pop("capture_session_generation", UNSET)
        )

        def _parse_color_space(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        color_space = _parse_color_space(d.pop("color_space", UNSET))

        def _parse_display_scale(data: object) -> float | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(float | None | Unset, data)

        display_scale = _parse_display_scale(d.pop("display_scale", UNSET))

        def _parse_dynamic_range(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        dynamic_range = _parse_dynamic_range(d.pop("dynamic_range", UNSET))

        def _parse_fallback_reason(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        fallback_reason = _parse_fallback_reason(d.pop("fallback_reason", UNSET))

        def _parse_native_height(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        native_height = _parse_native_height(d.pop("native_height", UNSET))

        def _parse_native_width(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        native_width = _parse_native_width(d.pop("native_width", UNSET))

        def _parse_owner_designated_requirement_hash(
            data: object,
        ) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        owner_designated_requirement_hash = _parse_owner_designated_requirement_hash(
            d.pop("owner_designated_requirement_hash", UNSET)
        )

        def _parse_pinned_generations(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        pinned_generations = _parse_pinned_generations(
            d.pop("pinned_generations", UNSET)
        )

        def _parse_pixel_format(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        pixel_format = _parse_pixel_format(d.pop("pixel_format", UNSET))

        def _parse_publication_path(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        publication_path = _parse_publication_path(d.pop("publication_path", UNSET))

        def _parse_publication_plan_generation(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        publication_plan_generation = _parse_publication_plan_generation(
            d.pop("publication_plan_generation", UNSET)
        )

        def _parse_resource_generation(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        resource_generation = _parse_resource_generation(
            d.pop("resource_generation", UNSET)
        )

        def _parse_selection_diagnostic_label(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        selection_diagnostic_label = _parse_selection_diagnostic_label(
            d.pop("selection_diagnostic_label", UNSET)
        )

        def _parse_timing(data: object) -> MacosScreenTimingApiStatus | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                timing_type_1 = MacosScreenTimingApiStatus.from_dict(data)

                return timing_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(MacosScreenTimingApiStatus | None | Unset, data)

        timing = _parse_timing(d.pop("timing", UNSET))

        def _parse_topology_generation(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        topology_generation = _parse_topology_generation(
            d.pop("topology_generation", UNSET)
        )

        def _parse_transfer_function(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        transfer_function = _parse_transfer_function(d.pop("transfer_function", UNSET))

        macos_screen_telemetry_api_status = cls(
            admitted_native_bytes=admitted_native_bytes,
            callback_max_ns=callback_max_ns,
            callback_total_ns=callback_total_ns,
            conversion_max_ns=conversion_max_ns,
            conversion_total_ns=conversion_total_ns,
            cpu_reduction_max_ns=cpu_reduction_max_ns,
            cpu_reduction_total_ns=cpu_reduction_total_ns,
            executable_architecture=executable_architecture,
            frames_dropped=frames_dropped,
            frames_malformed=frames_malformed,
            frames_published=frames_published,
            frames_received=frames_received,
            frames_stale=frames_stale,
            frames_superseded=frames_superseded,
            native_import_max_ns=native_import_max_ns,
            native_import_total_ns=native_import_total_ns,
            native_reduction_submit_max_ns=native_reduction_submit_max_ns,
            native_reduction_submit_total_ns=native_reduction_submit_total_ns,
            publication_max_ns=publication_max_ns,
            publication_total_ns=publication_total_ns,
            queue_depth=queue_depth,
            retain_max_ns=retain_max_ns,
            retain_total_ns=retain_total_ns,
            stream_state=stream_state,
            authorization_last_transition_age_ms=authorization_last_transition_age_ms,
            capture_session_generation=capture_session_generation,
            color_space=color_space,
            display_scale=display_scale,
            dynamic_range=dynamic_range,
            fallback_reason=fallback_reason,
            native_height=native_height,
            native_width=native_width,
            owner_designated_requirement_hash=owner_designated_requirement_hash,
            pinned_generations=pinned_generations,
            pixel_format=pixel_format,
            publication_path=publication_path,
            publication_plan_generation=publication_plan_generation,
            resource_generation=resource_generation,
            selection_diagnostic_label=selection_diagnostic_label,
            timing=timing,
            topology_generation=topology_generation,
            transfer_function=transfer_function,
        )

        macos_screen_telemetry_api_status.additional_properties = d
        return macos_screen_telemetry_api_status

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
