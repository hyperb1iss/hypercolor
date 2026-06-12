from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.api_response_active_effect_response_data_control_values import (
        ApiResponseActiveEffectResponseDataControlValues,
    )
    from ..models.control_definition import ControlDefinition


T = TypeVar("T", bound="ApiResponseActiveEffectResponseData")


@_attrs_define
class ApiResponseActiveEffectResponseData:
    """Response for `GET /api/v1/effects/active` — the primary zone's
    effect, or the idle shape (`state == "idle"`, `id`/`name` null).

        Attributes:
            state (str):
            active_preset_id (None | str | Unset):
            control_values (ApiResponseActiveEffectResponseDataControlValues | Unset):
            controls (list[ControlDefinition] | Unset):
            controls_version (int | None | Unset): Server-side version token for the group's controls. Clients
                that want to use optimistic concurrency on the effect-id PATCH
                endpoint echo this value back via `If-Match`. Idle responses
                omit it (there's nothing to version).
            cover_image_url (None | str | Unset):
            id (None | str | Unset):
            name (None | str | Unset):
            render_group_id (None | str | Unset):
    """

    state: str
    active_preset_id: None | str | Unset = UNSET
    control_values: ApiResponseActiveEffectResponseDataControlValues | Unset = UNSET
    controls: list[ControlDefinition] | Unset = UNSET
    controls_version: int | None | Unset = UNSET
    cover_image_url: None | str | Unset = UNSET
    id: None | str | Unset = UNSET
    name: None | str | Unset = UNSET
    render_group_id: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        state = self.state

        active_preset_id: None | str | Unset
        if isinstance(self.active_preset_id, Unset):
            active_preset_id = UNSET
        else:
            active_preset_id = self.active_preset_id

        control_values: dict[str, Any] | Unset = UNSET
        if not isinstance(self.control_values, Unset):
            control_values = self.control_values.to_dict()

        controls: list[dict[str, Any]] | Unset = UNSET
        if not isinstance(self.controls, Unset):
            controls = []
            for controls_item_data in self.controls:
                controls_item = controls_item_data.to_dict()
                controls.append(controls_item)

        controls_version: int | None | Unset
        if isinstance(self.controls_version, Unset):
            controls_version = UNSET
        else:
            controls_version = self.controls_version

        cover_image_url: None | str | Unset
        if isinstance(self.cover_image_url, Unset):
            cover_image_url = UNSET
        else:
            cover_image_url = self.cover_image_url

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
                "state": state,
            }
        )
        if active_preset_id is not UNSET:
            field_dict["active_preset_id"] = active_preset_id
        if control_values is not UNSET:
            field_dict["control_values"] = control_values
        if controls is not UNSET:
            field_dict["controls"] = controls
        if controls_version is not UNSET:
            field_dict["controls_version"] = controls_version
        if cover_image_url is not UNSET:
            field_dict["cover_image_url"] = cover_image_url
        if id is not UNSET:
            field_dict["id"] = id
        if name is not UNSET:
            field_dict["name"] = name
        if render_group_id is not UNSET:
            field_dict["render_group_id"] = render_group_id

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.api_response_active_effect_response_data_control_values import (
            ApiResponseActiveEffectResponseDataControlValues,
        )
        from ..models.control_definition import ControlDefinition

        d = dict(src_dict)
        state = d.pop("state")

        def _parse_active_preset_id(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        active_preset_id = _parse_active_preset_id(d.pop("active_preset_id", UNSET))

        _control_values = d.pop("control_values", UNSET)
        control_values: ApiResponseActiveEffectResponseDataControlValues | Unset
        if isinstance(_control_values, Unset):
            control_values = UNSET
        else:
            control_values = ApiResponseActiveEffectResponseDataControlValues.from_dict(
                _control_values
            )

        _controls = d.pop("controls", UNSET)
        controls: list[ControlDefinition] | Unset = UNSET
        if _controls is not UNSET:
            controls = []
            for controls_item_data in _controls:
                controls_item = ControlDefinition.from_dict(controls_item_data)

                controls.append(controls_item)

        def _parse_controls_version(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        controls_version = _parse_controls_version(d.pop("controls_version", UNSET))

        def _parse_cover_image_url(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        cover_image_url = _parse_cover_image_url(d.pop("cover_image_url", UNSET))

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

        api_response_active_effect_response_data = cls(
            state=state,
            active_preset_id=active_preset_id,
            control_values=control_values,
            controls=controls,
            controls_version=controls_version,
            cover_image_url=cover_image_url,
            id=id,
            name=name,
            render_group_id=render_group_id,
        )

        api_response_active_effect_response_data.additional_properties = d
        return api_response_active_effect_response_data

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
