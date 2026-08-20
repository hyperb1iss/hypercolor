from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.loop_mode import LoopMode
from ..types import UNSET, Unset

T = TypeVar("T", bound="MediaPlayback")


@_attrs_define
class MediaPlayback:
    """Media playback settings for media-backed layers.

    Attributes:
        auto_play (bool | Unset):
        loop_mode (LoopMode | Unset): End-of-stream policy for media playback.
        speed (float | Unset):
        start_offset_secs (float | Unset):
    """

    auto_play: bool | Unset = UNSET
    loop_mode: LoopMode | Unset = UNSET
    speed: float | Unset = UNSET
    start_offset_secs: float | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        auto_play = self.auto_play

        loop_mode: str | Unset = UNSET
        if not isinstance(self.loop_mode, Unset):
            loop_mode = self.loop_mode.value

        speed = self.speed

        start_offset_secs = self.start_offset_secs

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if auto_play is not UNSET:
            field_dict["auto_play"] = auto_play
        if loop_mode is not UNSET:
            field_dict["loop_mode"] = loop_mode
        if speed is not UNSET:
            field_dict["speed"] = speed
        if start_offset_secs is not UNSET:
            field_dict["start_offset_secs"] = start_offset_secs

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        auto_play = d.pop("auto_play", UNSET)

        _loop_mode = d.pop("loop_mode", UNSET)
        loop_mode: LoopMode | Unset
        if isinstance(_loop_mode, Unset):
            loop_mode = UNSET
        else:
            loop_mode = LoopMode(_loop_mode)

        speed = d.pop("speed", UNSET)

        start_offset_secs = d.pop("start_offset_secs", UNSET)

        media_playback = cls(
            auto_play=auto_play,
            loop_mode=loop_mode,
            speed=speed,
            start_offset_secs=start_offset_secs,
        )

        media_playback.additional_properties = d
        return media_playback

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
