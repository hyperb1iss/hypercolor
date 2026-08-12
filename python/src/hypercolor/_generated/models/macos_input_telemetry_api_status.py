from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.macos_architecture_api import MacosArchitectureApi
from ..types import UNSET, Unset

T = TypeVar("T", bound="MacosInputTelemetryApiStatus")


@_attrs_define
class MacosInputTelemetryApiStatus:
    """
    Attributes:
        executable_architecture (MacosArchitectureApi):
        authorization_last_transition_age_ms (int | None | Unset):
        capture_session_generation (int | None | Unset):
        host_architecture (MacosArchitectureApi | None | Unset):
        input_events_dropped (int | None | Unset):
        input_events_published (int | None | Unset):
        input_events_received (int | None | Unset):
        owner_designated_requirement_hash (None | str | Unset):
        queue_capacity (int | None | Unset):
        queue_depth (int | None | Unset):
        state_gaps (int | None | Unset):
        tap_disabled_timeout (int | None | Unset):
        tap_disabled_user_input (int | None | Unset):
        tap_reenabled (int | None | Unset):
        topology_generation (int | None | Unset):
        translated_process (bool | None | Unset):
    """

    executable_architecture: MacosArchitectureApi
    authorization_last_transition_age_ms: int | None | Unset = UNSET
    capture_session_generation: int | None | Unset = UNSET
    host_architecture: MacosArchitectureApi | None | Unset = UNSET
    input_events_dropped: int | None | Unset = UNSET
    input_events_published: int | None | Unset = UNSET
    input_events_received: int | None | Unset = UNSET
    owner_designated_requirement_hash: None | str | Unset = UNSET
    queue_capacity: int | None | Unset = UNSET
    queue_depth: int | None | Unset = UNSET
    state_gaps: int | None | Unset = UNSET
    tap_disabled_timeout: int | None | Unset = UNSET
    tap_disabled_user_input: int | None | Unset = UNSET
    tap_reenabled: int | None | Unset = UNSET
    topology_generation: int | None | Unset = UNSET
    translated_process: bool | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        executable_architecture = self.executable_architecture.value

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

        host_architecture: None | str | Unset
        if isinstance(self.host_architecture, Unset):
            host_architecture = UNSET
        elif isinstance(self.host_architecture, MacosArchitectureApi):
            host_architecture = self.host_architecture.value
        else:
            host_architecture = self.host_architecture

        input_events_dropped: int | None | Unset
        if isinstance(self.input_events_dropped, Unset):
            input_events_dropped = UNSET
        else:
            input_events_dropped = self.input_events_dropped

        input_events_published: int | None | Unset
        if isinstance(self.input_events_published, Unset):
            input_events_published = UNSET
        else:
            input_events_published = self.input_events_published

        input_events_received: int | None | Unset
        if isinstance(self.input_events_received, Unset):
            input_events_received = UNSET
        else:
            input_events_received = self.input_events_received

        owner_designated_requirement_hash: None | str | Unset
        if isinstance(self.owner_designated_requirement_hash, Unset):
            owner_designated_requirement_hash = UNSET
        else:
            owner_designated_requirement_hash = self.owner_designated_requirement_hash

        queue_capacity: int | None | Unset
        if isinstance(self.queue_capacity, Unset):
            queue_capacity = UNSET
        else:
            queue_capacity = self.queue_capacity

        queue_depth: int | None | Unset
        if isinstance(self.queue_depth, Unset):
            queue_depth = UNSET
        else:
            queue_depth = self.queue_depth

        state_gaps: int | None | Unset
        if isinstance(self.state_gaps, Unset):
            state_gaps = UNSET
        else:
            state_gaps = self.state_gaps

        tap_disabled_timeout: int | None | Unset
        if isinstance(self.tap_disabled_timeout, Unset):
            tap_disabled_timeout = UNSET
        else:
            tap_disabled_timeout = self.tap_disabled_timeout

        tap_disabled_user_input: int | None | Unset
        if isinstance(self.tap_disabled_user_input, Unset):
            tap_disabled_user_input = UNSET
        else:
            tap_disabled_user_input = self.tap_disabled_user_input

        tap_reenabled: int | None | Unset
        if isinstance(self.tap_reenabled, Unset):
            tap_reenabled = UNSET
        else:
            tap_reenabled = self.tap_reenabled

        topology_generation: int | None | Unset
        if isinstance(self.topology_generation, Unset):
            topology_generation = UNSET
        else:
            topology_generation = self.topology_generation

        translated_process: bool | None | Unset
        if isinstance(self.translated_process, Unset):
            translated_process = UNSET
        else:
            translated_process = self.translated_process

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "executable_architecture": executable_architecture,
            }
        )
        if authorization_last_transition_age_ms is not UNSET:
            field_dict["authorization_last_transition_age_ms"] = (
                authorization_last_transition_age_ms
            )
        if capture_session_generation is not UNSET:
            field_dict["capture_session_generation"] = capture_session_generation
        if host_architecture is not UNSET:
            field_dict["host_architecture"] = host_architecture
        if input_events_dropped is not UNSET:
            field_dict["input_events_dropped"] = input_events_dropped
        if input_events_published is not UNSET:
            field_dict["input_events_published"] = input_events_published
        if input_events_received is not UNSET:
            field_dict["input_events_received"] = input_events_received
        if owner_designated_requirement_hash is not UNSET:
            field_dict["owner_designated_requirement_hash"] = (
                owner_designated_requirement_hash
            )
        if queue_capacity is not UNSET:
            field_dict["queue_capacity"] = queue_capacity
        if queue_depth is not UNSET:
            field_dict["queue_depth"] = queue_depth
        if state_gaps is not UNSET:
            field_dict["state_gaps"] = state_gaps
        if tap_disabled_timeout is not UNSET:
            field_dict["tap_disabled_timeout"] = tap_disabled_timeout
        if tap_disabled_user_input is not UNSET:
            field_dict["tap_disabled_user_input"] = tap_disabled_user_input
        if tap_reenabled is not UNSET:
            field_dict["tap_reenabled"] = tap_reenabled
        if topology_generation is not UNSET:
            field_dict["topology_generation"] = topology_generation
        if translated_process is not UNSET:
            field_dict["translated_process"] = translated_process

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        executable_architecture = MacosArchitectureApi(d.pop("executable_architecture"))

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

        def _parse_host_architecture(
            data: object,
        ) -> MacosArchitectureApi | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                host_architecture_type_1 = MacosArchitectureApi(data)

                return host_architecture_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(MacosArchitectureApi | None | Unset, data)

        host_architecture = _parse_host_architecture(d.pop("host_architecture", UNSET))

        def _parse_input_events_dropped(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        input_events_dropped = _parse_input_events_dropped(
            d.pop("input_events_dropped", UNSET)
        )

        def _parse_input_events_published(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        input_events_published = _parse_input_events_published(
            d.pop("input_events_published", UNSET)
        )

        def _parse_input_events_received(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        input_events_received = _parse_input_events_received(
            d.pop("input_events_received", UNSET)
        )

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

        def _parse_queue_capacity(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        queue_capacity = _parse_queue_capacity(d.pop("queue_capacity", UNSET))

        def _parse_queue_depth(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        queue_depth = _parse_queue_depth(d.pop("queue_depth", UNSET))

        def _parse_state_gaps(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        state_gaps = _parse_state_gaps(d.pop("state_gaps", UNSET))

        def _parse_tap_disabled_timeout(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        tap_disabled_timeout = _parse_tap_disabled_timeout(
            d.pop("tap_disabled_timeout", UNSET)
        )

        def _parse_tap_disabled_user_input(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        tap_disabled_user_input = _parse_tap_disabled_user_input(
            d.pop("tap_disabled_user_input", UNSET)
        )

        def _parse_tap_reenabled(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        tap_reenabled = _parse_tap_reenabled(d.pop("tap_reenabled", UNSET))

        def _parse_topology_generation(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        topology_generation = _parse_topology_generation(
            d.pop("topology_generation", UNSET)
        )

        def _parse_translated_process(data: object) -> bool | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(bool | None | Unset, data)

        translated_process = _parse_translated_process(
            d.pop("translated_process", UNSET)
        )

        macos_input_telemetry_api_status = cls(
            executable_architecture=executable_architecture,
            authorization_last_transition_age_ms=authorization_last_transition_age_ms,
            capture_session_generation=capture_session_generation,
            host_architecture=host_architecture,
            input_events_dropped=input_events_dropped,
            input_events_published=input_events_published,
            input_events_received=input_events_received,
            owner_designated_requirement_hash=owner_designated_requirement_hash,
            queue_capacity=queue_capacity,
            queue_depth=queue_depth,
            state_gaps=state_gaps,
            tap_disabled_timeout=tap_disabled_timeout,
            tap_disabled_user_input=tap_disabled_user_input,
            tap_reenabled=tap_reenabled,
            topology_generation=topology_generation,
            translated_process=translated_process,
        )

        macos_input_telemetry_api_status.additional_properties = d
        return macos_input_telemetry_api_status

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
