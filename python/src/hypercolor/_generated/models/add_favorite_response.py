from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.favorite_summary import FavoriteSummary


T = TypeVar("T", bound="AddFavoriteResponse")


@_attrs_define
class AddFavoriteResponse:
    """Response for `POST /api/v1/library/favorites`.

    `created` is false when the effect was already favorited, which
    re-stamps `added_at_ms` rather than erroring.

        Attributes:
            created (bool):
            favorite (FavoriteSummary): One favorited effect.

                `effect_name` is resolved from the registry at request time and falls
                back to the id when the effect is no longer installed.
    """

    created: bool
    favorite: FavoriteSummary
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        created = self.created

        favorite = self.favorite.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "created": created,
                "favorite": favorite,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.favorite_summary import FavoriteSummary

        d = dict(src_dict)
        created = d.pop("created")

        favorite = FavoriteSummary.from_dict(d.pop("favorite"))

        add_favorite_response = cls(
            created=created,
            favorite=favorite,
        )

        add_favorite_response.additional_properties = d
        return add_favorite_response

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
