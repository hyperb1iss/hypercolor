from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

T = TypeVar("T", bound="HealthChecks")


@_attrs_define
class HealthChecks:
    """
    Attributes:
        device_backends (str):
        event_bus (str):
        render_loop (str):
    """

    device_backends: str
    event_bus: str
    render_loop: str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        device_backends = self.device_backends

        event_bus = self.event_bus

        render_loop = self.render_loop

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "device_backends": device_backends,
                "event_bus": event_bus,
                "render_loop": render_loop,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        device_backends = d.pop("device_backends")

        event_bus = d.pop("event_bus")

        render_loop = d.pop("render_loop")

        health_checks = cls(
            device_backends=device_backends,
            event_bus=event_bus,
            render_loop=render_loop,
        )

        health_checks.additional_properties = d
        return health_checks

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
