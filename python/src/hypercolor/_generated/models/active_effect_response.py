from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.active_effect_response_control_values import (
        ActiveEffectResponseControlValues,
    )
    from ..models.control_definition import ControlDefinition


T = TypeVar("T", bound="ActiveEffectResponse")


@_attrs_define
class ActiveEffectResponse:
    """
    Attributes:
        control_values (ActiveEffectResponseControlValues):
        controls (list[ControlDefinition]):
        state (str):
        active_preset_id (None | str | Unset):
        controls_version (int | None | Unset): Server-side version token for the group's controls. Clients
            that want to use optimistic concurrency on the effect-id PATCH
            endpoint echo this value back via `If-Match`. Idle responses
            omit it (there's nothing to version).
        id (None | str | Unset):
        name (None | str | Unset):
        render_group_id (None | str | Unset):
    """

    control_values: ActiveEffectResponseControlValues
    controls: list[ControlDefinition]
    state: str
    active_preset_id: None | str | Unset = UNSET
    controls_version: int | None | Unset = UNSET
    id: None | str | Unset = UNSET
    name: None | str | Unset = UNSET
    render_group_id: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        control_values = self.control_values.to_dict()

        controls = []
        for controls_item_data in self.controls:
            controls_item = controls_item_data.to_dict()
            controls.append(controls_item)

        state = self.state

        active_preset_id: None | str | Unset
        if isinstance(self.active_preset_id, Unset):
            active_preset_id = UNSET
        else:
            active_preset_id = self.active_preset_id

        controls_version: int | None | Unset
        if isinstance(self.controls_version, Unset):
            controls_version = UNSET
        else:
            controls_version = self.controls_version

        id: None | str | Unset
        if isinstance(self.id, Unset):
            id = UNSET
        else:
            id = self.id

        name: None | str | Unset
        if isinstance(self.name, Unset):
            name = UNSET
        else:
            name = self.name

        render_group_id: None | str | Unset
        if isinstance(self.render_group_id, Unset):
            render_group_id = UNSET
        else:
            render_group_id = self.render_group_id

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "control_values": control_values,
                "controls": controls,
                "state": state,
            }
        )
        if active_preset_id is not UNSET:
            field_dict["active_preset_id"] = active_preset_id
        if controls_version is not UNSET:
            field_dict["controls_version"] = controls_version
        if id is not UNSET:
            field_dict["id"] = id
        if name is not UNSET:
            field_dict["name"] = name
        if render_group_id is not UNSET:
            field_dict["render_group_id"] = render_group_id

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.active_effect_response_control_values import (
            ActiveEffectResponseControlValues,
        )
        from ..models.control_definition import ControlDefinition

        d = dict(src_dict)
        control_values = ActiveEffectResponseControlValues.from_dict(
            d.pop("control_values")
        )

        controls = []
        _controls = d.pop("controls")
        for controls_item_data in _controls:
            controls_item = ControlDefinition.from_dict(controls_item_data)

            controls.append(controls_item)

        state = d.pop("state")

        def _parse_active_preset_id(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        active_preset_id = _parse_active_preset_id(d.pop("active_preset_id", UNSET))

        def _parse_controls_version(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        controls_version = _parse_controls_version(d.pop("controls_version", UNSET))

        def _parse_id(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        id = _parse_id(d.pop("id", UNSET))

        def _parse_name(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        name = _parse_name(d.pop("name", UNSET))

        def _parse_render_group_id(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        render_group_id = _parse_render_group_id(d.pop("render_group_id", UNSET))

        active_effect_response = cls(
            control_values=control_values,
            controls=controls,
            state=state,
            active_preset_id=active_preset_id,
            controls_version=controls_version,
            id=id,
            name=name,
            render_group_id=render_group_id,
        )

        active_effect_response.additional_properties = d
        return active_effect_response

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
