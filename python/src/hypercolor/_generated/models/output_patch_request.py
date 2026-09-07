from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define

from ..models.output_power_mode import OutputPowerMode
from ..types import UNSET, Unset

T = TypeVar("T", bound="OutputPatchRequest")


@_attrs_define
class OutputPatchRequest:
    """`PATCH /api/v1/output` — partial: either or both fields.

    The range bound on `brightness` is a domain rule, not a parse rule:
    the service rejects an out-of-range value as a validation error so
    the caller gets a named field back instead of a decoder complaint.

        Attributes:
            brightness (float | None | Unset):
            power (None | OutputPowerMode | Unset):
    """

    brightness: float | None | Unset = UNSET
    power: None | OutputPowerMode | Unset = UNSET

    def to_dict(self) -> dict[str, Any]:
        brightness: float | None | Unset
        if isinstance(self.brightness, Unset):
            brightness = UNSET
        else:
            brightness = self.brightness

        power: None | str | Unset
        if isinstance(self.power, Unset):
            power = UNSET
        elif isinstance(self.power, OutputPowerMode):
            power = self.power.value
        else:
            power = self.power

        field_dict: dict[str, Any] = {}

        field_dict.update({})
        if brightness is not UNSET:
            field_dict["brightness"] = brightness
        if power is not UNSET:
            field_dict["power"] = power

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)

        def _parse_brightness(data: object) -> float | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(float | None | Unset, data)

        brightness = _parse_brightness(d.pop("brightness", UNSET))

        def _parse_power(data: object) -> None | OutputPowerMode | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                power_type_1 = OutputPowerMode(data)

                return power_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | OutputPowerMode | Unset, data)

        power = _parse_power(d.pop("power", UNSET))

        output_patch_request = cls(
            brightness=brightness,
            power=power,
        )

        return output_patch_request
