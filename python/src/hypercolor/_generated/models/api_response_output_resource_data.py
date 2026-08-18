from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.output_power_mode import OutputPowerMode

T = TypeVar("T", bound="ApiResponseOutputResourceData")


@_attrs_define
class ApiResponseOutputResourceData:
    """The one output resource — `GET /api/v1/output`.

    Attributes:
        brightness (float): Global brightness, `0.0..=1.0`.
        power (OutputPowerMode): Global output power state, both requested and observed.

            A destructive stop and a session sleep both read as `Paused`: the
            resource says whether output is running, and a stop's extra
            consequences are observable on the effect surface.
    """

    brightness: float
    power: OutputPowerMode
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        brightness = self.brightness

        power = self.power.value

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "brightness": brightness,
                "power": power,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        brightness = d.pop("brightness")

        power = OutputPowerMode(d.pop("power"))

        api_response_output_resource_data = cls(
            brightness=brightness,
            power=power,
        )

        api_response_output_resource_data.additional_properties = d
        return api_response_output_resource_data

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
