from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.effect_ref_summary import EffectRefSummary


T = TypeVar("T", bound="ApiResponsePauseEffectResponseData")


@_attrs_define
class ApiResponsePauseEffectResponseData:
    """Compatibility response for `POST /api/v1/effects/pause`.

    Attributes:
        off_output_behavior (str):
        off_output_color (list[int]):
        paused (bool):
        effect (EffectRefSummary | None | Unset):
    """

    off_output_behavior: str
    off_output_color: list[int]
    paused: bool
    effect: EffectRefSummary | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.effect_ref_summary import EffectRefSummary

        off_output_behavior = self.off_output_behavior

        off_output_color = self.off_output_color

        paused = self.paused

        effect: dict[str, Any] | None | Unset
        if isinstance(self.effect, Unset):
            effect = UNSET
        elif isinstance(self.effect, EffectRefSummary):
            effect = self.effect.to_dict()
        else:
            effect = self.effect

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "off_output_behavior": off_output_behavior,
                "off_output_color": off_output_color,
                "paused": paused,
            }
        )
        if effect is not UNSET:
            field_dict["effect"] = effect

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.effect_ref_summary import EffectRefSummary

        d = dict(src_dict)
        off_output_behavior = d.pop("off_output_behavior")

        off_output_color = cast(list[int], d.pop("off_output_color"))

        paused = d.pop("paused")

        def _parse_effect(data: object) -> EffectRefSummary | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                effect_type_1 = EffectRefSummary.from_dict(data)

                return effect_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(EffectRefSummary | None | Unset, data)

        effect = _parse_effect(d.pop("effect", UNSET))

        api_response_pause_effect_response_data = cls(
            off_output_behavior=off_output_behavior,
            off_output_color=off_output_color,
            paused=paused,
            effect=effect,
        )

        api_response_pause_effect_response_data.additional_properties = d
        return api_response_pause_effect_response_data

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
