from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.b_tree_map import BTreeMap
    from ..models.control_action_descriptor import ControlActionDescriptor
    from ..models.control_field_descriptor import ControlFieldDescriptor
    from ..models.control_group_descriptor import ControlGroupDescriptor
    from ..models.control_surface_document_values import ControlSurfaceDocumentValues
    from ..models.control_surface_scope import ControlSurfaceScope


T = TypeVar("T", bound="ControlSurfaceDocument")


@_attrs_define
class ControlSurfaceDocument:
    """Complete API document for a driver or device control surface.

    Attributes:
        actions (list[ControlActionDescriptor]): Action descriptors.
        availability (BTreeMap):
        fields (list[ControlFieldDescriptor]): Field descriptors.
        groups (list[ControlGroupDescriptor]): Semantic groups for fields and actions.
        revision (int):
        schema_version (int): Control-surface schema version.
        scope (ControlSurfaceScope): Scope owned by a control surface.
        surface_id (str):
        values (ControlSurfaceDocumentValues): Current field values keyed by field ID.
        action_availability (BTreeMap | Unset):
    """

    actions: list[ControlActionDescriptor]
    availability: BTreeMap
    fields: list[ControlFieldDescriptor]
    groups: list[ControlGroupDescriptor]
    revision: int
    schema_version: int
    scope: ControlSurfaceScope
    surface_id: str
    values: ControlSurfaceDocumentValues
    action_availability: BTreeMap | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        actions = []
        for actions_item_data in self.actions:
            actions_item = actions_item_data.to_dict()
            actions.append(actions_item)

        availability = self.availability.to_dict()

        fields = []
        for fields_item_data in self.fields:
            fields_item = fields_item_data.to_dict()
            fields.append(fields_item)

        groups = []
        for groups_item_data in self.groups:
            groups_item = groups_item_data.to_dict()
            groups.append(groups_item)

        revision = self.revision

        schema_version = self.schema_version

        scope = self.scope.to_dict()

        surface_id = self.surface_id

        values = self.values.to_dict()

        action_availability: dict[str, Any] | Unset = UNSET
        if not isinstance(self.action_availability, Unset):
            action_availability = self.action_availability.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "actions": actions,
                "availability": availability,
                "fields": fields,
                "groups": groups,
                "revision": revision,
                "schema_version": schema_version,
                "scope": scope,
                "surface_id": surface_id,
                "values": values,
            }
        )
        if action_availability is not UNSET:
            field_dict["action_availability"] = action_availability

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.b_tree_map import BTreeMap
        from ..models.control_action_descriptor import ControlActionDescriptor
        from ..models.control_field_descriptor import ControlFieldDescriptor
        from ..models.control_group_descriptor import ControlGroupDescriptor
        from ..models.control_surface_document_values import (
            ControlSurfaceDocumentValues,
        )
        from ..models.control_surface_scope import ControlSurfaceScope

        d = dict(src_dict)
        actions = []
        _actions = d.pop("actions")
        for actions_item_data in _actions:
            actions_item = ControlActionDescriptor.from_dict(actions_item_data)

            actions.append(actions_item)

        availability = BTreeMap.from_dict(d.pop("availability"))

        fields = []
        _fields = d.pop("fields")
        for fields_item_data in _fields:
            fields_item = ControlFieldDescriptor.from_dict(fields_item_data)

            fields.append(fields_item)

        groups = []
        _groups = d.pop("groups")
        for groups_item_data in _groups:
            groups_item = ControlGroupDescriptor.from_dict(groups_item_data)

            groups.append(groups_item)

        revision = d.pop("revision")

        schema_version = d.pop("schema_version")

        scope = ControlSurfaceScope.from_dict(d.pop("scope"))

        surface_id = d.pop("surface_id")

        values = ControlSurfaceDocumentValues.from_dict(d.pop("values"))

        _action_availability = d.pop("action_availability", UNSET)
        action_availability: BTreeMap | Unset
        if isinstance(_action_availability, Unset):
            action_availability = UNSET
        else:
            action_availability = BTreeMap.from_dict(_action_availability)

        control_surface_document = cls(
            actions=actions,
            availability=availability,
            fields=fields,
            groups=groups,
            revision=revision,
            schema_version=schema_version,
            scope=scope,
            surface_id=surface_id,
            values=values,
            action_availability=action_availability,
        )

        control_surface_document.additional_properties = d
        return control_surface_document

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
