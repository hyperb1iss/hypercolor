from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.apply_effect_request_controls import ApplyEffectRequestControls
    from ..models.transition_request import TransitionRequest


T = TypeVar("T", bound="ApplyEffectRequest")


@_attrs_define
class ApplyEffectRequest:
    """
    Attributes:
        controls (ApplyEffectRequestControls):
        preset_id (None | str | Unset): Optional preset ID to associate with the render group in the same
            transaction as the effect start — lets the UI pass a remembered
            preset selection without a follow-up round-trip. If `controls` is
            also provided, the explicit controls win (they're presumed to
            already carry the preset's values, possibly with user tweaks).
        transition (None | TransitionRequest | Unset):
    """

    controls: ApplyEffectRequestControls
    preset_id: None | str | Unset = UNSET
    transition: None | TransitionRequest | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.transition_request import TransitionRequest

        controls = self.controls.to_dict()

        preset_id: None | str | Unset
        if isinstance(self.preset_id, Unset):
            preset_id = UNSET
        else:
            preset_id = self.preset_id

        transition: dict[str, Any] | None | Unset
        if isinstance(self.transition, Unset):
            transition = UNSET
        elif isinstance(self.transition, TransitionRequest):
            transition = self.transition.to_dict()
        else:
            transition = self.transition

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "controls": controls,
            }
        )
        if preset_id is not UNSET:
            field_dict["preset_id"] = preset_id
        if transition is not UNSET:
            field_dict["transition"] = transition

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.apply_effect_request_controls import ApplyEffectRequestControls
        from ..models.transition_request import TransitionRequest

        d = dict(src_dict)
        controls = ApplyEffectRequestControls.from_dict(d.pop("controls"))

        def _parse_preset_id(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        preset_id = _parse_preset_id(d.pop("preset_id", UNSET))

        def _parse_transition(data: object) -> None | TransitionRequest | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                transition_type_1 = TransitionRequest.from_dict(data)

                return transition_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | TransitionRequest | Unset, data)

        transition = _parse_transition(d.pop("transition", UNSET))

        apply_effect_request = cls(
            controls=controls,
            preset_id=preset_id,
            transition=transition,
        )

        apply_effect_request.additional_properties = d
        return apply_effect_request

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
