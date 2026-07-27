from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.input_source_status import InputSourceStatus


T = TypeVar("T", bound="InputStatus")


@_attrs_define
class InputStatus:
    """Host keyboard/mouse capture health, for consent and remediation UX.

    `enabled` is the consent config gate. `host_capturing` is true when a
    host backend is actively reading device nodes. `devices_denied` counts
    input nodes present but unreadable (udev rules missing) — the signal
    that distinguishes "input is off" from "input is on but blocked".

    `degraded` carries the failures the counters cannot express. Windows has no
    per-device denial to count: either the process has a visible window station
    and sees input, or it does not, and that is a session-level fact rather than
    a per-node one.

        Attributes:
            backends (list[str]):
            devices_denied (int):
            devices_opened (int):
            enabled (bool):
            host_capture_registered (bool):
            host_capturing (bool):
            degraded (None | str | Unset):
            source_graph_generation (int | Unset):
            sources (list[InputSourceStatus] | Unset):
    """

    backends: list[str]
    devices_denied: int
    devices_opened: int
    enabled: bool
    host_capture_registered: bool
    host_capturing: bool
    degraded: None | str | Unset = UNSET
    source_graph_generation: int | Unset = UNSET
    sources: list[InputSourceStatus] | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        backends = self.backends

        devices_denied = self.devices_denied

        devices_opened = self.devices_opened

        enabled = self.enabled

        host_capture_registered = self.host_capture_registered

        host_capturing = self.host_capturing

        degraded: None | str | Unset
        if isinstance(self.degraded, Unset):
            degraded = UNSET
        else:
            degraded = self.degraded

        source_graph_generation = self.source_graph_generation

        sources: list[dict[str, Any]] | Unset = UNSET
        if not isinstance(self.sources, Unset):
            sources = []
            for sources_item_data in self.sources:
                sources_item = sources_item_data.to_dict()
                sources.append(sources_item)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "backends": backends,
                "devices_denied": devices_denied,
                "devices_opened": devices_opened,
                "enabled": enabled,
                "host_capture_registered": host_capture_registered,
                "host_capturing": host_capturing,
            }
        )
        if degraded is not UNSET:
            field_dict["degraded"] = degraded
        if source_graph_generation is not UNSET:
            field_dict["source_graph_generation"] = source_graph_generation
        if sources is not UNSET:
            field_dict["sources"] = sources

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.input_source_status import InputSourceStatus

        d = dict(src_dict)
        backends = cast(list[str], d.pop("backends"))

        devices_denied = d.pop("devices_denied")

        devices_opened = d.pop("devices_opened")

        enabled = d.pop("enabled")

        host_capture_registered = d.pop("host_capture_registered")

        host_capturing = d.pop("host_capturing")

        def _parse_degraded(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        degraded = _parse_degraded(d.pop("degraded", UNSET))

        source_graph_generation = d.pop("source_graph_generation", UNSET)

        _sources = d.pop("sources", UNSET)
        sources: list[InputSourceStatus] | Unset = UNSET
        if _sources is not UNSET:
            sources = []
            for sources_item_data in _sources:
                sources_item = InputSourceStatus.from_dict(sources_item_data)

                sources.append(sources_item)

        input_status = cls(
            backends=backends,
            devices_denied=devices_denied,
            devices_opened=devices_opened,
            enabled=enabled,
            host_capture_registered=host_capture_registered,
            host_capturing=host_capturing,
            degraded=degraded,
            source_graph_generation=source_graph_generation,
            sources=sources,
        )

        input_status.additional_properties = d
        return input_status

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
