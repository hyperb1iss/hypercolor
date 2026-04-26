from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.viewport_rect import ViewportRect


T = TypeVar("T", bound="ControlValueType7")


@_attrs_define
class ControlValueType7:
    """Normalized rectangular viewport.

    Attributes:
        rect (ViewportRect): Normalized viewport rectangle in `[0.0, 1.0]` source space.
    """

    rect: ViewportRect
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        rect = self.rect.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "rect": rect,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.viewport_rect import ViewportRect

        d = dict(src_dict)
        rect = ViewportRect.from_dict(d.pop("rect"))

        control_value_type_7 = cls(
            rect=rect,
        )

        control_value_type_7.additional_properties = d
        return control_value_type_7

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
