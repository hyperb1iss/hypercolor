from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.sensor_reading import SensorReading


T = TypeVar("T", bound="SystemSnapshot")


@_attrs_define
class SystemSnapshot:
    """Published system snapshot shared across render, API, and overlays.

    Attributes:
        components (list[SensorReading]): Raw component readings collected from the host.
        cpu_load_percent (float): Aggregate CPU load across all cores (0.0–100.0).
        cpu_loads (list[float]): Per-core CPU load percentages.
        polled_at_ms (int): Unix timestamp in milliseconds when the snapshot was polled.
        ram_total_mb (float): RAM total in megabytes.
        ram_used_mb (float): RAM used in megabytes.
        ram_used_percent (float): RAM usage percentage (0.0–100.0).
        cpu_temp_celsius (float | None | Unset): CPU package temperature, if available.
        gpu_load_percent (float | None | Unset): GPU load percentage, if available.
        gpu_temp_celsius (float | None | Unset): GPU temperature, if available.
        gpu_vram_used_mb (float | None | Unset): GPU VRAM used in megabytes, if available.
    """

    components: list[SensorReading]
    cpu_load_percent: float
    cpu_loads: list[float]
    polled_at_ms: int
    ram_total_mb: float
    ram_used_mb: float
    ram_used_percent: float
    cpu_temp_celsius: float | None | Unset = UNSET
    gpu_load_percent: float | None | Unset = UNSET
    gpu_temp_celsius: float | None | Unset = UNSET
    gpu_vram_used_mb: float | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        components = []
        for components_item_data in self.components:
            components_item = components_item_data.to_dict()
            components.append(components_item)

        cpu_load_percent = self.cpu_load_percent

        cpu_loads = self.cpu_loads

        polled_at_ms = self.polled_at_ms

        ram_total_mb = self.ram_total_mb

        ram_used_mb = self.ram_used_mb

        ram_used_percent = self.ram_used_percent

        cpu_temp_celsius: float | None | Unset
        if isinstance(self.cpu_temp_celsius, Unset):
            cpu_temp_celsius = UNSET
        else:
            cpu_temp_celsius = self.cpu_temp_celsius

        gpu_load_percent: float | None | Unset
        if isinstance(self.gpu_load_percent, Unset):
            gpu_load_percent = UNSET
        else:
            gpu_load_percent = self.gpu_load_percent

        gpu_temp_celsius: float | None | Unset
        if isinstance(self.gpu_temp_celsius, Unset):
            gpu_temp_celsius = UNSET
        else:
            gpu_temp_celsius = self.gpu_temp_celsius

        gpu_vram_used_mb: float | None | Unset
        if isinstance(self.gpu_vram_used_mb, Unset):
            gpu_vram_used_mb = UNSET
        else:
            gpu_vram_used_mb = self.gpu_vram_used_mb

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "components": components,
                "cpu_load_percent": cpu_load_percent,
                "cpu_loads": cpu_loads,
                "polled_at_ms": polled_at_ms,
                "ram_total_mb": ram_total_mb,
                "ram_used_mb": ram_used_mb,
                "ram_used_percent": ram_used_percent,
            }
        )
        if cpu_temp_celsius is not UNSET:
            field_dict["cpu_temp_celsius"] = cpu_temp_celsius
        if gpu_load_percent is not UNSET:
            field_dict["gpu_load_percent"] = gpu_load_percent
        if gpu_temp_celsius is not UNSET:
            field_dict["gpu_temp_celsius"] = gpu_temp_celsius
        if gpu_vram_used_mb is not UNSET:
            field_dict["gpu_vram_used_mb"] = gpu_vram_used_mb

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.sensor_reading import SensorReading

        d = dict(src_dict)
        components = []
        _components = d.pop("components")
        for components_item_data in _components:
            components_item = SensorReading.from_dict(components_item_data)

            components.append(components_item)

        cpu_load_percent = d.pop("cpu_load_percent")

        cpu_loads = cast(list[float], d.pop("cpu_loads"))

        polled_at_ms = d.pop("polled_at_ms")

        ram_total_mb = d.pop("ram_total_mb")

        ram_used_mb = d.pop("ram_used_mb")

        ram_used_percent = d.pop("ram_used_percent")

        def _parse_cpu_temp_celsius(data: object) -> float | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(float | None | Unset, data)

        cpu_temp_celsius = _parse_cpu_temp_celsius(d.pop("cpu_temp_celsius", UNSET))

        def _parse_gpu_load_percent(data: object) -> float | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(float | None | Unset, data)

        gpu_load_percent = _parse_gpu_load_percent(d.pop("gpu_load_percent", UNSET))

        def _parse_gpu_temp_celsius(data: object) -> float | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(float | None | Unset, data)

        gpu_temp_celsius = _parse_gpu_temp_celsius(d.pop("gpu_temp_celsius", UNSET))

        def _parse_gpu_vram_used_mb(data: object) -> float | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(float | None | Unset, data)

        gpu_vram_used_mb = _parse_gpu_vram_used_mb(d.pop("gpu_vram_used_mb", UNSET))

        system_snapshot = cls(
            components=components,
            cpu_load_percent=cpu_load_percent,
            cpu_loads=cpu_loads,
            polled_at_ms=polled_at_ms,
            ram_total_mb=ram_total_mb,
            ram_used_mb=ram_used_mb,
            ram_used_percent=ram_used_percent,
            cpu_temp_celsius=cpu_temp_celsius,
            gpu_load_percent=gpu_load_percent,
            gpu_temp_celsius=gpu_temp_celsius,
            gpu_vram_used_mb=gpu_vram_used_mb,
        )

        system_snapshot.additional_properties = d
        return system_snapshot

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
