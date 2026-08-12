from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="ApplyEffectPresetRequest")


@_attrs_define
class ApplyEffectPresetRequest:
    """Optional body for `POST /api/v1/effects/{id}/presets/{preset_id}/apply`.

    Attributes:
        render_group (None | str | Unset):
    """

    render_group: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        render_group: None | str | Unset
        if isinstance(self.render_group, Unset):
            render_group = UNSET
        else:
            render_group = self.render_group

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if render_group is not UNSET:
            field_dict["render_group"] = render_group

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)

        def _parse_render_group(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        render_group = _parse_render_group(d.pop("render_group", UNSET))

        apply_effect_preset_request = cls(
            render_group=render_group,
        )

        apply_effect_preset_request.additional_properties = d
        return apply_effect_preset_request

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
