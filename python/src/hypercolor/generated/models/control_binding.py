from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="ControlBinding")


@_attrs_define
class ControlBinding:
    """Live mapping from a system sensor reading into a control value.

    Attributes:
        sensor (str): Stable sensor label to sample from the current system snapshot.
        sensor_max (float): Upper bound of the source sensor range.
        sensor_min (float): Lower bound of the source sensor range.
        target_max (float): Upper bound of the mapped control range.
        target_min (float): Lower bound of the mapped control range.
        deadband (float | Unset): Minimum source-value delta required before the binding updates.
        smoothing (float | Unset): Temporal smoothing factor. `0.0` is immediate, `0.99` is very slow.
    """

    sensor: str
    sensor_max: float
    sensor_min: float
    target_max: float
    target_min: float
    deadband: float | Unset = UNSET
    smoothing: float | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        sensor = self.sensor

        sensor_max = self.sensor_max

        sensor_min = self.sensor_min

        target_max = self.target_max

        target_min = self.target_min

        deadband = self.deadband

        smoothing = self.smoothing

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "sensor": sensor,
                "sensor_max": sensor_max,
                "sensor_min": sensor_min,
                "target_max": target_max,
                "target_min": target_min,
            }
        )
        if deadband is not UNSET:
            field_dict["deadband"] = deadband
        if smoothing is not UNSET:
            field_dict["smoothing"] = smoothing

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        sensor = d.pop("sensor")

        sensor_max = d.pop("sensor_max")

        sensor_min = d.pop("sensor_min")

        target_max = d.pop("target_max")

        target_min = d.pop("target_min")

        deadband = d.pop("deadband", UNSET)

        smoothing = d.pop("smoothing", UNSET)

        control_binding = cls(
            sensor=sensor,
            sensor_max=sensor_max,
            sensor_min=sensor_min,
            target_max=target_max,
            target_min=target_min,
            deadband=deadband,
            smoothing=smoothing,
        )

        control_binding.additional_properties = d
        return control_binding

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
