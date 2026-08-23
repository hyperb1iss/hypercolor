from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.active_playlist_response import ActivePlaylistResponse


T = TypeVar("T", bound="ActivePlaylistStateResponse")


@_attrs_define
class ActivePlaylistStateResponse:
    """Response for `GET /api/v1/library/playlists/active`.

    The route answers 404 when nothing is playing, so `state` is always
    `"running"` on a success.

        Attributes:
            playlist (ActivePlaylistResponse): The playlist the daemon is currently cycling through.

                This is the live runtime's view, not the stored playlist: the item
                list is reduced to `item_count`, and `started_at_ms` is when playback
                began rather than when the playlist was saved.
            state (str | Unset):
    """

    playlist: ActivePlaylistResponse
    state: str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        playlist = self.playlist.to_dict()

        state = self.state

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "playlist": playlist,
            }
        )
        if state is not UNSET:
            field_dict["state"] = state

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.active_playlist_response import ActivePlaylistResponse

        d = dict(src_dict)
        playlist = ActivePlaylistResponse.from_dict(d.pop("playlist"))

        state = d.pop("state", UNSET)

        active_playlist_state_response = cls(
            playlist=playlist,
            state=state,
        )

        active_playlist_state_response.additional_properties = d
        return active_playlist_state_response

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
