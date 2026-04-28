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
from ..models.control_access import ControlAccess
from ..models.control_persistence import ControlPersistence
from ..models.control_visibility import ControlVisibility
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.apply_impact_type_7 import ApplyImpactType7
    from ..models.control_field_descriptor_availability import (
        ControlFieldDescriptorAvailability,
    )
    from ..models.control_field_descriptor_default_value_type_0 import (
        ControlFieldDescriptorDefaultValueType0,
    )
    from ..models.control_field_descriptor_value_type import (
        ControlFieldDescriptorValueType,
    )
    from ..models.control_owner import ControlOwner


T = TypeVar("T", bound="ControlFieldDescriptor")


@_attrs_define
class ControlFieldDescriptor:
    """Field descriptor for one typed control.

    Attributes:
        access (ControlAccess): Field access mode.
        apply_impact (ApplyImpactType0 | ApplyImpactType1 | ApplyImpactType2 | ApplyImpactType3 | ApplyImpactType4 |
            ApplyImpactType5 | ApplyImpactType6 | ApplyImpactType7): Dynamic impact required to apply a control change.
        availability (ControlFieldDescriptorAvailability): Availability expression before daemon resolution.
        id (str):
        label (str): Human-readable label.
        ordering (int): Stable ordering hint.
        owner (ControlOwner): Field, action, or group owner.
        persistence (ControlPersistence): Persistence target for a control field.
        value_type (ControlFieldDescriptorValueType): Expected value type.
        visibility (ControlVisibility): Field visibility tier.
        default_value (ControlFieldDescriptorDefaultValueType0 | None | Unset): Optional default value.
        description (None | str | Unset): Optional help text.
        group_id (None | str | Unset):
    """

    access: ControlAccess
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
    availability: ControlFieldDescriptorAvailability
    id: str
    label: str
    ordering: int
    owner: ControlOwner
    persistence: ControlPersistence
    value_type: ControlFieldDescriptorValueType
    visibility: ControlVisibility
    default_value: ControlFieldDescriptorDefaultValueType0 | None | Unset = UNSET
    description: None | str | Unset = UNSET
    group_id: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.control_field_descriptor_default_value_type_0 import (
            ControlFieldDescriptorDefaultValueType0,
        )

        access = self.access.value

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

        label = self.label

        ordering = self.ordering

        owner = self.owner.to_dict()

        persistence = self.persistence.value

        value_type = self.value_type.to_dict()

        visibility = self.visibility.value

        default_value: dict[str, Any] | None | Unset
        if isinstance(self.default_value, Unset):
            default_value = UNSET
        elif isinstance(self.default_value, ControlFieldDescriptorDefaultValueType0):
            default_value = self.default_value.to_dict()
        else:
            default_value = self.default_value

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

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "access": access,
                "apply_impact": apply_impact,
                "availability": availability,
                "id": id,
                "label": label,
                "ordering": ordering,
                "owner": owner,
                "persistence": persistence,
                "value_type": value_type,
                "visibility": visibility,
            }
        )
        if default_value is not UNSET:
            field_dict["default_value"] = default_value
        if description is not UNSET:
            field_dict["description"] = description
        if group_id is not UNSET:
            field_dict["group_id"] = group_id

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.apply_impact_type_7 import ApplyImpactType7
        from ..models.control_field_descriptor_availability import (
            ControlFieldDescriptorAvailability,
        )
        from ..models.control_field_descriptor_default_value_type_0 import (
            ControlFieldDescriptorDefaultValueType0,
        )
        from ..models.control_field_descriptor_value_type import (
            ControlFieldDescriptorValueType,
        )
        from ..models.control_owner import ControlOwner

        d = dict(src_dict)
        access = ControlAccess(d.pop("access"))

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

        availability = ControlFieldDescriptorAvailability.from_dict(
            d.pop("availability")
        )

        id = d.pop("id")

        label = d.pop("label")

        ordering = d.pop("ordering")

        owner = ControlOwner.from_dict(d.pop("owner"))

        persistence = ControlPersistence(d.pop("persistence"))

        value_type = ControlFieldDescriptorValueType.from_dict(d.pop("value_type"))

        visibility = ControlVisibility(d.pop("visibility"))

        def _parse_default_value(
            data: object,
        ) -> ControlFieldDescriptorDefaultValueType0 | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                default_value_type_0 = (
                    ControlFieldDescriptorDefaultValueType0.from_dict(data)
                )

                return default_value_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(ControlFieldDescriptorDefaultValueType0 | None | Unset, data)

        default_value = _parse_default_value(d.pop("default_value", UNSET))

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

        control_field_descriptor = cls(
            access=access,
            apply_impact=apply_impact,
            availability=availability,
            id=id,
            label=label,
            ordering=ordering,
            owner=owner,
            persistence=persistence,
            value_type=value_type,
            visibility=visibility,
            default_value=default_value,
            description=description,
            group_id=group_id,
        )

        control_field_descriptor.additional_properties = d
        return control_field_descriptor

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
