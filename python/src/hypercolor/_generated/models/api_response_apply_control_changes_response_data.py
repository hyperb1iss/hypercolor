from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.apply_impact_type_0 import ApplyImpactType0
from ..models.apply_impact_type_1 import ApplyImpactType1
from ..models.apply_impact_type_2 import ApplyImpactType2
from ..models.apply_impact_type_3 import ApplyImpactType3
from ..models.apply_impact_type_4 import ApplyImpactType4
from ..models.apply_impact_type_5 import ApplyImpactType5
from ..models.apply_impact_type_6 import ApplyImpactType6

if TYPE_CHECKING:
    from ..models.api_response_apply_control_changes_response_data_values import (
        ApiResponseApplyControlChangesResponseDataValues,
    )
    from ..models.applied_control_change import AppliedControlChange
    from ..models.apply_impact_type_7 import ApplyImpactType7
    from ..models.rejected_control_change import RejectedControlChange


T = TypeVar("T", bound="ApiResponseApplyControlChangesResponseData")


@_attrs_define
class ApiResponseApplyControlChangesResponseData:
    """Response from applying control changes.

    Attributes:
        accepted (list[AppliedControlChange]): Accepted changes after driver normalization.
        impacts (list[ApplyImpactType0 | ApplyImpactType1 | ApplyImpactType2 | ApplyImpactType3 | ApplyImpactType4 |
            ApplyImpactType5 | ApplyImpactType6 | ApplyImpactType7]): Dynamic impacts produced by the transaction.
        previous_revision (int):
        rejected (list[RejectedControlChange]): Rejected changes.
        revision (int):
        surface_id (str):
        values (ApiResponseApplyControlChangesResponseDataValues): Current values after the transaction.
    """

    accepted: list[AppliedControlChange]
    impacts: list[
        ApplyImpactType0
        | ApplyImpactType1
        | ApplyImpactType2
        | ApplyImpactType3
        | ApplyImpactType4
        | ApplyImpactType5
        | ApplyImpactType6
        | ApplyImpactType7
    ]
    previous_revision: int
    rejected: list[RejectedControlChange]
    revision: int
    surface_id: str
    values: ApiResponseApplyControlChangesResponseDataValues
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
            elif isinstance(impacts_item_data, ApplyImpactType1):
                impacts_item = impacts_item_data.value
            elif isinstance(impacts_item_data, ApplyImpactType2):
                impacts_item = impacts_item_data.value
            elif isinstance(impacts_item_data, ApplyImpactType3):
                impacts_item = impacts_item_data.value
            elif isinstance(impacts_item_data, ApplyImpactType4):
                impacts_item = impacts_item_data.value
            elif isinstance(impacts_item_data, ApplyImpactType5):
                impacts_item = impacts_item_data.value
            elif isinstance(impacts_item_data, ApplyImpactType6):
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
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.api_response_apply_control_changes_response_data_values import (
            ApiResponseApplyControlChangesResponseDataValues,
        )
        from ..models.applied_control_change import AppliedControlChange
        from ..models.apply_impact_type_7 import ApplyImpactType7
        from ..models.rejected_control_change import RejectedControlChange

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
            ) -> (
                ApplyImpactType0
                | ApplyImpactType1
                | ApplyImpactType2
                | ApplyImpactType3
                | ApplyImpactType4
                | ApplyImpactType5
                | ApplyImpactType6
                | ApplyImpactType7
            ):
                try:
                    if not isinstance(data, str):
                        raise TypeError()
                    componentsschemas_apply_impact_type_0 = ApplyImpactType0(data)

                    return componentsschemas_apply_impact_type_0
                except (TypeError, ValueError, AttributeError, KeyError):
                    pass
                try:
                    if not isinstance(data, str):
                        raise TypeError()
                    componentsschemas_apply_impact_type_1 = ApplyImpactType1(data)

                    return componentsschemas_apply_impact_type_1
                except (TypeError, ValueError, AttributeError, KeyError):
                    pass
                try:
                    if not isinstance(data, str):
                        raise TypeError()
                    componentsschemas_apply_impact_type_2 = ApplyImpactType2(data)

                    return componentsschemas_apply_impact_type_2
                except (TypeError, ValueError, AttributeError, KeyError):
                    pass
                try:
                    if not isinstance(data, str):
                        raise TypeError()
                    componentsschemas_apply_impact_type_3 = ApplyImpactType3(data)

                    return componentsschemas_apply_impact_type_3
                except (TypeError, ValueError, AttributeError, KeyError):
                    pass
                try:
                    if not isinstance(data, str):
                        raise TypeError()
                    componentsschemas_apply_impact_type_4 = ApplyImpactType4(data)

                    return componentsschemas_apply_impact_type_4
                except (TypeError, ValueError, AttributeError, KeyError):
                    pass
                try:
                    if not isinstance(data, str):
                        raise TypeError()
                    componentsschemas_apply_impact_type_5 = ApplyImpactType5(data)

                    return componentsschemas_apply_impact_type_5
                except (TypeError, ValueError, AttributeError, KeyError):
                    pass
                try:
                    if not isinstance(data, str):
                        raise TypeError()
                    componentsschemas_apply_impact_type_6 = ApplyImpactType6(data)

                    return componentsschemas_apply_impact_type_6
                except (TypeError, ValueError, AttributeError, KeyError):
                    pass
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_apply_impact_type_7 = ApplyImpactType7.from_dict(data)

                return componentsschemas_apply_impact_type_7

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

        values = ApiResponseApplyControlChangesResponseDataValues.from_dict(
            d.pop("values")
        )

        api_response_apply_control_changes_response_data = cls(
            accepted=accepted,
            impacts=impacts,
            previous_revision=previous_revision,
            rejected=rejected,
            revision=revision,
            surface_id=surface_id,
            values=values,
        )

        api_response_apply_control_changes_response_data.additional_properties = d
        return api_response_apply_control_changes_response_data

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
