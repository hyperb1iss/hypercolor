from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.edge_behavior_fade_to_black_value import EdgeBehaviorFadeToBlackValue


T = TypeVar("T", bound="EdgeBehaviorFadeToBlack")


@_attrs_define
class EdgeBehaviorFadeToBlack:
    """Fade to black outside canvas bounds. `falloff` controls fade rate.

    Attributes:
        fade_to_black (EdgeBehaviorFadeToBlackValue): Fade to black outside canvas bounds. `falloff` controls fade rate.
    """

    fade_to_black: EdgeBehaviorFadeToBlackValue
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        fade_to_black = self.fade_to_black.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "fade_to_black": fade_to_black,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.edge_behavior_fade_to_black_value import (
            EdgeBehaviorFadeToBlackValue,
        )

        d = dict(src_dict)
        fade_to_black = EdgeBehaviorFadeToBlackValue.from_dict(d.pop("fade_to_black"))

        edge_behavior_fade_to_black = cls(
            fade_to_black=fade_to_black,
        )

        edge_behavior_fade_to_black.additional_properties = d
        return edge_behavior_fade_to_black

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
