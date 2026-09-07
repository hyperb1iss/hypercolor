from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.spatial_layout import SpatialLayout


T = TypeVar("T", bound="ApplyLayoutResponse")


@_attrs_define
class ApplyLayoutResponse:
    """Response from `POST /api/v1/layouts/{id}/apply`.

    Attributes:
        applied (bool):
        layout (SpatialLayout): Top-level spatial layout container.

            Defines the complete mapping from a 2D effect canvas to the physical LED
            positions of every connected device. All coordinates use normalized
            `[0.0, 1.0]` space where `(0,0)` is top-left and `(1,1)` is bottom-right.
        persistence_pending (bool):
    """

    applied: bool
    layout: SpatialLayout
    persistence_pending: bool
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        applied = self.applied

        layout = self.layout.to_dict()

        persistence_pending = self.persistence_pending

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "applied": applied,
                "layout": layout,
                "persistence_pending": persistence_pending,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.spatial_layout import SpatialLayout

        d = dict(src_dict)
        applied = d.pop("applied")

        layout = SpatialLayout.from_dict(d.pop("layout"))

        persistence_pending = d.pop("persistence_pending")

        apply_layout_response = cls(
            applied=applied,
            layout=layout,
            persistence_pending=persistence_pending,
        )

        apply_layout_response.additional_properties = d
        return apply_layout_response

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
