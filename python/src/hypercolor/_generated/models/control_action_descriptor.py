from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.apply_impact_type_0 import ApplyImpactType0
from ..models.apply_impact_type_1 import ApplyImpactType1
from ..models.apply_impact_type_2 import ApplyImpactType2
from ..models.apply_impact_type_3 import ApplyImpactType3
from ..models.apply_impact_type_4 import ApplyImpactType4
from ..models.apply_impact_type_5 import ApplyImpactType5
from ..models.apply_impact_type_6 import ApplyImpactType6
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.action_confirmation import ActionConfirmation
    from ..models.apply_impact_type_7 import ApplyImpactType7
    from ..models.control_action_descriptor_availability import (
        ControlActionDescriptorAvailability,
    )
    from ..models.control_action_descriptor_result_type_type_0 import (
        ControlActionDescriptorResultTypeType0,
    )
    from ..models.control_object_field import ControlObjectField
    from ..models.control_owner import ControlOwner


T = TypeVar("T", bound="ControlActionDescriptor")


@_attrs_define
class ControlActionDescriptor:
    """Action descriptor for one-shot commands.

    Attributes:
        apply_impact (ApplyImpactType0 | ApplyImpactType1 | ApplyImpactType2 | ApplyImpactType3 | ApplyImpactType4 |
            ApplyImpactType5 | ApplyImpactType6 | ApplyImpactType7): Dynamic impact required to apply a control change.
        availability (ControlActionDescriptorAvailability): Availability expression before daemon resolution.
        id (str):
        input_fields (list[ControlObjectField]): Typed input fields.
        label (str): Human-readable label.
        ordering (int): Stable ordering hint.
        owner (ControlOwner): Field, action, or group owner.
        confirmation (ActionConfirmation | None | Unset):
        description (None | str | Unset): Optional help text.
        group_id (None | str | Unset):
        result_type (ControlActionDescriptorResultTypeType0 | None | Unset): Optional typed result.
    """

    apply_impact: (
        ApplyImpactType0
        | ApplyImpactType1
        | ApplyImpactType2
        | ApplyImpactType3
        | ApplyImpactType4
        | ApplyImpactType5
        | ApplyImpactType6
        | ApplyImpactType7
    )
    availability: ControlActionDescriptorAvailability
    id: str
    input_fields: list[ControlObjectField]
    label: str
    ordering: int
    owner: ControlOwner
    confirmation: ActionConfirmation | None | Unset = UNSET
    description: None | str | Unset = UNSET
    group_id: None | str | Unset = UNSET
    result_type: ControlActionDescriptorResultTypeType0 | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.action_confirmation import ActionConfirmation
        from ..models.control_action_descriptor_result_type_type_0 import (
            ControlActionDescriptorResultTypeType0,
        )

        apply_impact: dict[str, Any] | str
        if isinstance(self.apply_impact, ApplyImpactType0):
            apply_impact = self.apply_impact.value
        elif isinstance(self.apply_impact, ApplyImpactType1):
            apply_impact = self.apply_impact.value
        elif isinstance(self.apply_impact, ApplyImpactType2):
            apply_impact = self.apply_impact.value
        elif isinstance(self.apply_impact, ApplyImpactType3):
            apply_impact = self.apply_impact.value
        elif isinstance(self.apply_impact, ApplyImpactType4):
            apply_impact = self.apply_impact.value
        elif isinstance(self.apply_impact, ApplyImpactType5):
            apply_impact = self.apply_impact.value
        elif isinstance(self.apply_impact, ApplyImpactType6):
            apply_impact = self.apply_impact.value
        else:
            apply_impact = self.apply_impact.to_dict()

        availability = self.availability.to_dict()

        id = self.id

        input_fields = []
        for input_fields_item_data in self.input_fields:
            input_fields_item = input_fields_item_data.to_dict()
            input_fields.append(input_fields_item)

        label = self.label

        ordering = self.ordering

        owner = self.owner.to_dict()

        confirmation: dict[str, Any] | None | Unset
        if isinstance(self.confirmation, Unset):
            confirmation = UNSET
        elif isinstance(self.confirmation, ActionConfirmation):
            confirmation = self.confirmation.to_dict()
        else:
            confirmation = self.confirmation

        description: None | str | Unset
        if isinstance(self.description, Unset):
            description = UNSET
        else:
            description = self.description

        group_id: None | str | Unset
        if isinstance(self.group_id, Unset):
            group_id = UNSET
        else:
            group_id = self.group_id

        result_type: dict[str, Any] | None | Unset
        if isinstance(self.result_type, Unset):
            result_type = UNSET
        elif isinstance(self.result_type, ControlActionDescriptorResultTypeType0):
            result_type = self.result_type.to_dict()
        else:
            result_type = self.result_type

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "apply_impact": apply_impact,
                "availability": availability,
                "id": id,
                "input_fields": input_fields,
                "label": label,
                "ordering": ordering,
                "owner": owner,
            }
        )
        if confirmation is not UNSET:
            field_dict["confirmation"] = confirmation
        if description is not UNSET:
            field_dict["description"] = description
        if group_id is not UNSET:
            field_dict["group_id"] = group_id
        if result_type is not UNSET:
            field_dict["result_type"] = result_type

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.action_confirmation import ActionConfirmation
        from ..models.apply_impact_type_7 import ApplyImpactType7
        from ..models.control_action_descriptor_availability import (
            ControlActionDescriptorAvailability,
        )
        from ..models.control_action_descriptor_result_type_type_0 import (
            ControlActionDescriptorResultTypeType0,
        )
        from ..models.control_object_field import ControlObjectField
        from ..models.control_owner import ControlOwner

        d = dict(src_dict)

        def _parse_apply_impact(
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

        apply_impact = _parse_apply_impact(d.pop("apply_impact"))

        availability = ControlActionDescriptorAvailability.from_dict(
            d.pop("availability")
        )

        id = d.pop("id")

        input_fields = []
        _input_fields = d.pop("input_fields")
        for input_fields_item_data in _input_fields:
            input_fields_item = ControlObjectField.from_dict(input_fields_item_data)

            input_fields.append(input_fields_item)

        label = d.pop("label")

        ordering = d.pop("ordering")

        owner = ControlOwner.from_dict(d.pop("owner"))

        def _parse_confirmation(data: object) -> ActionConfirmation | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                confirmation_type_1 = ActionConfirmation.from_dict(data)

                return confirmation_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(ActionConfirmation | None | Unset, data)

        confirmation = _parse_confirmation(d.pop("confirmation", UNSET))

        def _parse_description(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        description = _parse_description(d.pop("description", UNSET))

        def _parse_group_id(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        group_id = _parse_group_id(d.pop("group_id", UNSET))

        def _parse_result_type(
            data: object,
        ) -> ControlActionDescriptorResultTypeType0 | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                result_type_type_0 = ControlActionDescriptorResultTypeType0.from_dict(
                    data
                )

                return result_type_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(ControlActionDescriptorResultTypeType0 | None | Unset, data)

        result_type = _parse_result_type(d.pop("result_type", UNSET))

        control_action_descriptor = cls(
            apply_impact=apply_impact,
            availability=availability,
            id=id,
            input_fields=input_fields,
            label=label,
            ordering=ordering,
            owner=owner,
            confirmation=confirmation,
            description=description,
            group_id=group_id,
            result_type=result_type,
        )

        control_action_descriptor.additional_properties = d
        return control_action_descriptor

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
