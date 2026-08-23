from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.binding_source_type_2_kind import BindingSourceType2Kind
from ..models.time_wave import TimeWave

T = TypeVar("T", bound="BindingSourceType2")


@_attrs_define
class BindingSourceType2:
    """
    Attributes:
        kind (BindingSourceType2Kind):
        rate_hz (float):
        wave (TimeWave): Time-domain waveform for layer bindings.
    """

    kind: BindingSourceType2Kind
    rate_hz: float
    wave: TimeWave
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        kind = self.kind.value

        rate_hz = self.rate_hz

        wave = self.wave.value

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "kind": kind,
                "rate_hz": rate_hz,
                "wave": wave,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        kind = BindingSourceType2Kind(d.pop("kind"))

        rate_hz = d.pop("rate_hz")

        wave = TimeWave(d.pop("wave"))

        binding_source_type_2 = cls(
            kind=kind,
            rate_hz=rate_hz,
            wave=wave,
        )

        binding_source_type_2.additional_properties = d
        return binding_source_type_2

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
