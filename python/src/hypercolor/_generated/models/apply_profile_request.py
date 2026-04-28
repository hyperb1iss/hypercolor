from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="ApplyProfileRequest")


@_attrs_define
class ApplyProfileRequest:
    """
    Attributes:
        transition_ms (int | None | Unset):
    """

    transition_ms: int | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        transition_ms: int | None | Unset
        if isinstance(self.transition_ms, Unset):
            transition_ms = UNSET
        else:
            transition_ms = self.transition_ms

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if transition_ms is not UNSET:
            field_dict["transition_ms"] = transition_ms

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)

        def _parse_transition_ms(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        transition_ms = _parse_transition_ms(d.pop("transition_ms", UNSET))

        apply_profile_request = cls(
            transition_ms=transition_ms,
        )

        apply_profile_request.additional_properties = d
        return apply_profile_request

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
