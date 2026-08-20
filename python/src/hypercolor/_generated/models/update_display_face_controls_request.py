from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="UpdateDisplayFaceControlsRequest")


@_attrs_define
class UpdateDisplayFaceControlsRequest:
    """Request body for `PATCH /api/v1/displays/{id}/face/controls`.

    The payload carries only the overrides the caller wants to change;
    existing control values on the zone are preserved unless their
    key appears in this map. `controls` is typed as raw JSON (rather than
    `HashMap<String, ControlValue>`) so callers can send natural shapes
    like `{"accent": 0.5}` instead of `{"accent": {"float": 0.5}}`, which
    mirrors the effects controls patch endpoint.

        Attributes:
            controls (Any | Unset):
    """

    controls: Any | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        controls = self.controls

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if controls is not UNSET:
            field_dict["controls"] = controls

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        controls = d.pop("controls", UNSET)

        update_display_face_controls_request = cls(
            controls=controls,
        )

        update_display_face_controls_request.additional_properties = d
        return update_display_face_controls_request

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
