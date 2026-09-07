from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.audio_band import AudioBand
from ..models.binding_source_type_0_kind import BindingSourceType0Kind

T = TypeVar("T", bound="BindingSourceType0")


@_attrs_define
class BindingSourceType0:
    """
    Attributes:
        band (AudioBand): Coarse audio features exposed to layer bindings.
        kind (BindingSourceType0Kind):
    """

    band: AudioBand
    kind: BindingSourceType0Kind
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        band = self.band.value

        kind = self.kind.value

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "band": band,
                "kind": kind,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        band = AudioBand(d.pop("band"))

        kind = BindingSourceType0Kind(d.pop("kind"))

        binding_source_type_0 = cls(
            band=band,
            kind=kind,
        )

        binding_source_type_0.additional_properties = d
        return binding_source_type_0

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
