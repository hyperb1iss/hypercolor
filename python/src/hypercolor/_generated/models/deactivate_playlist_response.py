from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.active_playlist_response import ActivePlaylistResponse


T = TypeVar("T", bound="DeactivatePlaylistResponse")


@_attrs_define
class DeactivatePlaylistResponse:
    """Response for `POST /api/v1/library/playlists/deactivate`.

    Attributes:
        deactivated (bool):
        playlist (ActivePlaylistResponse): The playlist the daemon is currently cycling through.

            This is the live runtime's view, not the stored playlist: the item
            list is reduced to `item_count`, and `started_at_ms` is when playback
            began rather than when the playlist was saved.
    """

    deactivated: bool
    playlist: ActivePlaylistResponse
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        deactivated = self.deactivated

        playlist = self.playlist.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "deactivated": deactivated,
                "playlist": playlist,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.active_playlist_response import ActivePlaylistResponse

        d = dict(src_dict)
        deactivated = d.pop("deactivated")

        playlist = ActivePlaylistResponse.from_dict(d.pop("playlist"))

        deactivate_playlist_response = cls(
            deactivated=deactivated,
            playlist=playlist,
        )

        deactivate_playlist_response.additional_properties = d
        return deactivate_playlist_response

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
