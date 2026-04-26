from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.apply_effect_response_applied_controls import (
        ApplyEffectResponseAppliedControls,
    )
    from ..models.apply_transition_response import ApplyTransitionResponse
    from ..models.effect_layout_apply_result import EffectLayoutApplyResult
    from ..models.effect_ref_summary import EffectRefSummary


T = TypeVar("T", bound="ApplyEffectResponse")


@_attrs_define
class ApplyEffectResponse:
    """
    Attributes:
        applied_controls (ApplyEffectResponseAppliedControls):
        effect (EffectRefSummary):
        transition (ApplyTransitionResponse):
        warnings (list[str]):
        layout (EffectLayoutApplyResult | None | Unset):
    """

    applied_controls: ApplyEffectResponseAppliedControls
    effect: EffectRefSummary
    transition: ApplyTransitionResponse
    warnings: list[str]
    layout: EffectLayoutApplyResult | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.effect_layout_apply_result import EffectLayoutApplyResult

        applied_controls = self.applied_controls.to_dict()

        effect = self.effect.to_dict()

        transition = self.transition.to_dict()

        warnings = self.warnings

        layout: dict[str, Any] | None | Unset
        if isinstance(self.layout, Unset):
            layout = UNSET
        elif isinstance(self.layout, EffectLayoutApplyResult):
            layout = self.layout.to_dict()
        else:
            layout = self.layout

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "applied_controls": applied_controls,
                "effect": effect,
                "transition": transition,
                "warnings": warnings,
            }
        )
        if layout is not UNSET:
            field_dict["layout"] = layout

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.apply_effect_response_applied_controls import (
            ApplyEffectResponseAppliedControls,
        )
        from ..models.apply_transition_response import ApplyTransitionResponse
        from ..models.effect_layout_apply_result import EffectLayoutApplyResult
        from ..models.effect_ref_summary import EffectRefSummary

        d = dict(src_dict)
        applied_controls = ApplyEffectResponseAppliedControls.from_dict(
            d.pop("applied_controls")
        )

        effect = EffectRefSummary.from_dict(d.pop("effect"))

        transition = ApplyTransitionResponse.from_dict(d.pop("transition"))

        warnings = cast(list[str], d.pop("warnings"))

        def _parse_layout(data: object) -> EffectLayoutApplyResult | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                layout_type_1 = EffectLayoutApplyResult.from_dict(data)

                return layout_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(EffectLayoutApplyResult | None | Unset, data)

        layout = _parse_layout(d.pop("layout", UNSET))

        apply_effect_response = cls(
            applied_controls=applied_controls,
            effect=effect,
            transition=transition,
            warnings=warnings,
            layout=layout,
        )

        apply_effect_response.additional_properties = d
        return apply_effect_response

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
