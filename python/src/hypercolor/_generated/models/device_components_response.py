from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.component_binding_summary import ComponentBindingSummary
    from ..models.component_slot import ComponentSlot
    from ..models.component_suggested_zone import ComponentSuggestedZone


T = TypeVar("T", bound="DeviceComponentsResponse")


@_attrs_define
class DeviceComponentsResponse:
    """Response for `GET /api/v1/devices/{id}/attachments`.

    `slots` are the controller's physical attachment points, `bindings`
    what is attached to them, and `suggested_zones` the layout zones the
    attachments imply.

        Attributes:
            device_id (str):
            device_name (str):
            bindings (list[ComponentBindingSummary] | Unset):
            slots (list[ComponentSlot] | Unset):
            suggested_zones (list[ComponentSuggestedZone] | Unset):
    """

    device_id: str
    device_name: str
    bindings: list[ComponentBindingSummary] | Unset = UNSET
    slots: list[ComponentSlot] | Unset = UNSET
    suggested_zones: list[ComponentSuggestedZone] | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        device_id = self.device_id

        device_name = self.device_name

        bindings: list[dict[str, Any]] | Unset = UNSET
        if not isinstance(self.bindings, Unset):
            bindings = []
            for bindings_item_data in self.bindings:
                bindings_item = bindings_item_data.to_dict()
                bindings.append(bindings_item)

        slots: list[dict[str, Any]] | Unset = UNSET
        if not isinstance(self.slots, Unset):
            slots = []
            for slots_item_data in self.slots:
                slots_item = slots_item_data.to_dict()
                slots.append(slots_item)

        suggested_zones: list[dict[str, Any]] | Unset = UNSET
        if not isinstance(self.suggested_zones, Unset):
            suggested_zones = []
            for suggested_zones_item_data in self.suggested_zones:
                suggested_zones_item = suggested_zones_item_data.to_dict()
                suggested_zones.append(suggested_zones_item)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "device_id": device_id,
                "device_name": device_name,
            }
        )
        if bindings is not UNSET:
            field_dict["bindings"] = bindings
        if slots is not UNSET:
            field_dict["slots"] = slots
        if suggested_zones is not UNSET:
            field_dict["suggested_zones"] = suggested_zones

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.component_binding_summary import ComponentBindingSummary
        from ..models.component_slot import ComponentSlot
        from ..models.component_suggested_zone import ComponentSuggestedZone

        d = dict(src_dict)
        device_id = d.pop("device_id")

        device_name = d.pop("device_name")

        _bindings = d.pop("bindings", UNSET)
        bindings: list[ComponentBindingSummary] | Unset = UNSET
        if _bindings is not UNSET:
            bindings = []
            for bindings_item_data in _bindings:
                bindings_item = ComponentBindingSummary.from_dict(bindings_item_data)

                bindings.append(bindings_item)

        _slots = d.pop("slots", UNSET)
        slots: list[ComponentSlot] | Unset = UNSET
        if _slots is not UNSET:
            slots = []
            for slots_item_data in _slots:
                slots_item = ComponentSlot.from_dict(slots_item_data)

                slots.append(slots_item)

        _suggested_zones = d.pop("suggested_zones", UNSET)
        suggested_zones: list[ComponentSuggestedZone] | Unset = UNSET
        if _suggested_zones is not UNSET:
            suggested_zones = []
            for suggested_zones_item_data in _suggested_zones:
                suggested_zones_item = ComponentSuggestedZone.from_dict(
                    suggested_zones_item_data
                )

                suggested_zones.append(suggested_zones_item)

        device_components_response = cls(
            device_id=device_id,
            device_name=device_name,
            bindings=bindings,
            slots=slots,
            suggested_zones=suggested_zones,
        )

        device_components_response.additional_properties = d
        return device_components_response

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
