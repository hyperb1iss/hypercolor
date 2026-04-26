from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.pairing_flow_kind import PairingFlowKind
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.pairing_field_descriptor import PairingFieldDescriptor


T = TypeVar("T", bound="PairingDescriptor")


@_attrs_define
class PairingDescriptor:
    """Backend-provided pairing UI/CLI descriptor.

    Attributes:
        action_label (str):
        instructions (list[str]):
        kind (PairingFlowKind): How the UI or CLI should present a pairing flow.
        title (str):
        fields (list[PairingFieldDescriptor] | Unset):
    """

    action_label: str
    instructions: list[str]
    kind: PairingFlowKind
    title: str
    fields: list[PairingFieldDescriptor] | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        action_label = self.action_label

        instructions = self.instructions

        kind = self.kind.value

        title = self.title

        fields: list[dict[str, Any]] | Unset = UNSET
        if not isinstance(self.fields, Unset):
            fields = []
            for fields_item_data in self.fields:
                fields_item = fields_item_data.to_dict()
                fields.append(fields_item)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "action_label": action_label,
                "instructions": instructions,
                "kind": kind,
                "title": title,
            }
        )
        if fields is not UNSET:
            field_dict["fields"] = fields

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.pairing_field_descriptor import PairingFieldDescriptor

        d = dict(src_dict)
        action_label = d.pop("action_label")

        instructions = cast(list[str], d.pop("instructions"))

        kind = PairingFlowKind(d.pop("kind"))

        title = d.pop("title")

        _fields = d.pop("fields", UNSET)
        fields: list[PairingFieldDescriptor] | Unset = UNSET
        if _fields is not UNSET:
            fields = []
            for fields_item_data in _fields:
                fields_item = PairingFieldDescriptor.from_dict(fields_item_data)

                fields.append(fields_item)

        pairing_descriptor = cls(
            action_label=action_label,
            instructions=instructions,
            kind=kind,
            title=title,
            fields=fields,
        )

        pairing_descriptor.additional_properties = d
        return pairing_descriptor

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
