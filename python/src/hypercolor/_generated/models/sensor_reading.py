from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.sensor_unit import SensorUnit
from ..types import UNSET, Unset

T = TypeVar("T", bound="SensorReading")


@_attrs_define
class SensorReading:
    """A single host sensor reading.

    Attributes:
        label (str): Stable sensor label.
        unit (SensorUnit): Units exposed by system sensors.
        value (float): Current sensor value.
        critical (float | None | Unset): Critical threshold, if known.
        max_ (float | None | Unset): Expected maximum value, if known.
        min_ (float | None | Unset): Expected minimum value, if known.
    """

    label: str
    unit: SensorUnit
    value: float
    critical: float | None | Unset = UNSET
    max_: float | None | Unset = UNSET
    min_: float | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        label = self.label

        unit = self.unit.value

        value = self.value

        critical: float | None | Unset
        if isinstance(self.critical, Unset):
            critical = UNSET
        else:
            critical = self.critical

        max_: float | None | Unset
        if isinstance(self.max_, Unset):
            max_ = UNSET
        else:
            max_ = self.max_

        min_: float | None | Unset
        if isinstance(self.min_, Unset):
            min_ = UNSET
        else:
            min_ = self.min_

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "label": label,
                "unit": unit,
                "value": value,
            }
        )
        if critical is not UNSET:
            field_dict["critical"] = critical
        if max_ is not UNSET:
            field_dict["max"] = max_
        if min_ is not UNSET:
            field_dict["min"] = min_

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        label = d.pop("label")

        unit = SensorUnit(d.pop("unit"))

        value = d.pop("value")

        def _parse_critical(data: object) -> float | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(float | None | Unset, data)

        critical = _parse_critical(d.pop("critical", UNSET))

        def _parse_max_(data: object) -> float | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(float | None | Unset, data)

        max_ = _parse_max_(d.pop("max", UNSET))

        def _parse_min_(data: object) -> float | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(float | None | Unset, data)

        min_ = _parse_min_(d.pop("min", UNSET))

        sensor_reading = cls(
            label=label,
            unit=unit,
            value=value,
            critical=critical,
            max_=max_,
            min_=min_,
        )

        sensor_reading.additional_properties = d
        return sensor_reading

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
