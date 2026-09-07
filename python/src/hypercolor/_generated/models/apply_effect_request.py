from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast
from uuid import UUID

from attrs import define as _attrs_define

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.apply_effect_request_controls_type_0 import (
        ApplyEffectRequestControlsType0,
    )
    from ..models.transition_type_type_0 import TransitionTypeType0


T = TypeVar("T", bound="ApplyEffectRequest")


@_attrs_define
class ApplyEffectRequest:
    """`POST /effects/{id}/apply` — the sugar request (Spec 78 §2.3).

    Replaces the target zone's layer stack with a single new layer
    running this effect; a projection of the same `SceneMutation` a
    layer-stack replacement performs, never a second code path.

        Attributes:
            controls (ApplyEffectRequestControlsType0 | None | Unset):
            preset_id (None | Unset | UUID):
            transition (None | TransitionTypeType0 | Unset):
            zone (None | str | Unset): Target zone; omitted means the primary zone, created if the
                scene has none.
    """

    controls: ApplyEffectRequestControlsType0 | None | Unset = UNSET
    preset_id: None | Unset | UUID = UNSET
    transition: None | TransitionTypeType0 | Unset = UNSET
    zone: None | str | Unset = UNSET

    def to_dict(self) -> dict[str, Any]:
        from ..models.apply_effect_request_controls_type_0 import (
            ApplyEffectRequestControlsType0,
        )
        from ..models.transition_type_type_0 import TransitionTypeType0

        controls: dict[str, Any] | None | Unset
        if isinstance(self.controls, Unset):
            controls = UNSET
        elif isinstance(self.controls, ApplyEffectRequestControlsType0):
            controls = self.controls.to_dict()
        else:
            controls = self.controls

        preset_id: None | str | Unset
        if isinstance(self.preset_id, Unset):
            preset_id = UNSET
        elif isinstance(self.preset_id, UUID):
            preset_id = str(self.preset_id)
        else:
            preset_id = self.preset_id

        transition: dict[str, Any] | None | Unset
        if isinstance(self.transition, Unset):
            transition = UNSET
        elif isinstance(self.transition, TransitionTypeType0):
            transition = self.transition.to_dict()
        else:
            transition = self.transition

        zone: None | str | Unset
        if isinstance(self.zone, Unset):
            zone = UNSET
        else:
            zone = self.zone

        field_dict: dict[str, Any] = {}

        field_dict.update({})
        if controls is not UNSET:
            field_dict["controls"] = controls
        if preset_id is not UNSET:
            field_dict["preset_id"] = preset_id
        if transition is not UNSET:
            field_dict["transition"] = transition
        if zone is not UNSET:
            field_dict["zone"] = zone

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.apply_effect_request_controls_type_0 import (
            ApplyEffectRequestControlsType0,
        )
        from ..models.transition_type_type_0 import TransitionTypeType0

        d = dict(src_dict)

        def _parse_controls(
            data: object,
        ) -> ApplyEffectRequestControlsType0 | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                controls_type_0 = ApplyEffectRequestControlsType0.from_dict(data)

                return controls_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(ApplyEffectRequestControlsType0 | None | Unset, data)

        controls = _parse_controls(d.pop("controls", UNSET))

        def _parse_preset_id(data: object) -> None | Unset | UUID:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                preset_id_type_1 = UUID(data)

                return preset_id_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | Unset | UUID, data)

        preset_id = _parse_preset_id(d.pop("preset_id", UNSET))

        def _parse_transition(data: object) -> None | TransitionTypeType0 | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_transition_type_type_0 = (
                    TransitionTypeType0.from_dict(data)
                )

                return componentsschemas_transition_type_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | TransitionTypeType0 | Unset, data)

        transition = _parse_transition(d.pop("transition", UNSET))

        def _parse_zone(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        zone = _parse_zone(d.pop("zone", UNSET))

        apply_effect_request = cls(
            controls=controls,
            preset_id=preset_id,
            transition=transition,
            zone=zone,
        )

        return apply_effect_request
