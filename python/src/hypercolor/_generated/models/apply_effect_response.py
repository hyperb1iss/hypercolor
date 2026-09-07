from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.side_effect_outcome import SideEffectOutcome
    from ..models.transition_type_type_0 import TransitionTypeType0
    from ..models.zone_resource import ZoneResource


T = TypeVar("T", bound="ApplyEffectResponse")


@_attrs_define
class ApplyEffectResponse:
    """`POST /effects/{id}/apply` — the sugar response: the updated zone
    resource carrying the new layer's id, and the applied transition.

    Post-commit side-effect failures (power wake) are reported inside a
    200 per Spec 78 §2.3; repair goes through the side effect's own
    route (`PATCH /output`), never a blind re-apply, because apply
    mints a fresh layer id and is deliberately not idempotent.

        Attributes:
            output (SideEffectOutcome): One post-commit side-effect outcome (Spec 78 §2.3, §3.2): the
                commit stands, the outcome says whether the side effect landed,
                and a failure carries its reason.
            transition (TransitionTypeType0): The closed transition vocabulary (Spec 78 §2.3).

                Grows when the engine does; the request field does not accept
                aspirational values.
            zone (ZoneResource): One authored zone inside the live document (Spec 78 §1.3).
    """

    output: SideEffectOutcome
    transition: TransitionTypeType0
    zone: ZoneResource
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.transition_type_type_0 import TransitionTypeType0

        output = self.output.to_dict()

        transition: dict[str, Any]
        if isinstance(self.transition, TransitionTypeType0):
            transition = self.transition.to_dict()

        zone = self.zone.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "output": output,
                "transition": transition,
                "zone": zone,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.side_effect_outcome import SideEffectOutcome
        from ..models.transition_type_type_0 import TransitionTypeType0
        from ..models.zone_resource import ZoneResource

        d = dict(src_dict)
        output = SideEffectOutcome.from_dict(d.pop("output"))

        def _parse_transition(data: object) -> TransitionTypeType0:
            if not isinstance(data, dict):
                raise TypeError()
            componentsschemas_transition_type_type_0 = TransitionTypeType0.from_dict(
                data
            )

            return componentsschemas_transition_type_type_0

        transition = _parse_transition(d.pop("transition"))

        zone = ZoneResource.from_dict(d.pop("zone"))

        apply_effect_response = cls(
            output=output,
            transition=transition,
            zone=zone,
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
