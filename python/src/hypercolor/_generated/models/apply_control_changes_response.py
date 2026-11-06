from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..models.apply_impact_type_0 import ApplyImpactType0

if TYPE_CHECKING:
    from ..models.applied_control_change import AppliedControlChange
    from ..models.apply_control_changes_response_values import (
        ApplyControlChangesResponseValues,
    )
    from ..models.apply_impact_type_1 import ApplyImpactType1
    from ..models.rejected_control_change import RejectedControlChange


T = TypeVar("T", bound="ApplyControlChangesResponse")


@_attrs_define
class ApplyControlChangesResponse:
    """Response from applying control changes.

    Attributes:
        accepted (list[AppliedControlChange]): Accepted changes after driver normalization.
        impacts (list[ApplyImpactType0 | ApplyImpactType1]): Dynamic impacts produced by the transaction.
        previous_revision (int):
        rejected (list[RejectedControlChange]): Rejected changes.
        revision (int):
        surface_id (str):
        values (ApplyControlChangesResponseValues): Current values after the transaction.
    """

    accepted: list[AppliedControlChange]
    impacts: list[ApplyImpactType0 | ApplyImpactType1]
    previous_revision: int
    rejected: list[RejectedControlChange]
    revision: int
    surface_id: str
    values: ApplyControlChangesResponseValues
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        accepted = []
        for accepted_item_data in self.accepted:
            accepted_item = accepted_item_data.to_dict()
            accepted.append(accepted_item)

        impacts = []
        for impacts_item_data in self.impacts:
            impacts_item: dict[str, Any] | str
            if isinstance(impacts_item_data, ApplyImpactType0):
                impacts_item = impacts_item_data.value
            else:
                impacts_item = impacts_item_data.to_dict()

            impacts.append(impacts_item)

        previous_revision = self.previous_revision

        rejected = []
        for rejected_item_data in self.rejected:
            rejected_item = rejected_item_data.to_dict()
            rejected.append(rejected_item)

        revision = self.revision

        surface_id = self.surface_id

        values = self.values.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "accepted": accepted,
                "impacts": impacts,
                "previous_revision": previous_revision,
                "rejected": rejected,
                "revision": revision,
                "surface_id": surface_id,
                "values": values,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.applied_control_change import (
            AppliedControlChange,
        )
        from ..models.apply_control_changes_response_values import (
            ApplyControlChangesResponseValues,
        )
        from ..models.apply_impact_type_1 import ApplyImpactType1
        from ..models.rejected_control_change import (
            RejectedControlChange,
        )

        d = dict(src_dict)
        accepted = []
        _accepted = d.pop("accepted")
        for accepted_item_data in _accepted:
            accepted_item = AppliedControlChange.from_dict(accepted_item_data)

            accepted.append(accepted_item)

        impacts = []
        _impacts = d.pop("impacts")
        for impacts_item_data in _impacts:

            def _parse_impacts_item(
                data: object,
            ) -> ApplyImpactType0 | ApplyImpactType1:
                try:
                    if not isinstance(data, str):
                        raise TypeError()
                    componentsschemas_apply_impact_type_0 = ApplyImpactType0(data)

                    return componentsschemas_apply_impact_type_0
                except (TypeError, ValueError, AttributeError, KeyError):
                    pass
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_apply_impact_type_1 = ApplyImpactType1.from_dict(data)

                return componentsschemas_apply_impact_type_1

            impacts_item = _parse_impacts_item(impacts_item_data)

            impacts.append(impacts_item)

        previous_revision = d.pop("previous_revision")

        rejected = []
        _rejected = d.pop("rejected")
        for rejected_item_data in _rejected:
            rejected_item = RejectedControlChange.from_dict(rejected_item_data)

            rejected.append(rejected_item)

        revision = d.pop("revision")

        surface_id = d.pop("surface_id")

        values = ApplyControlChangesResponseValues.from_dict(d.pop("values"))

        apply_control_changes_response = cls(
            accepted=accepted,
            impacts=impacts,
            previous_revision=previous_revision,
            rejected=rejected,
            revision=revision,
            surface_id=surface_id,
            values=values,
        )

        apply_control_changes_response.additional_properties = d
        return apply_control_changes_response

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
