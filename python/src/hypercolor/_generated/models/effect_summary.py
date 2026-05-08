from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="EffectSummary")


@_attrs_define
class EffectSummary:
    """
    Attributes:
        audio_reactive (bool):
        author (str):
        category (str):
        description (str):
        id (str):
        name (str):
        runnable (bool):
        source (str):
        tags (list[str]):
        version (str):
        cover_image_url (None | str | Unset):
    """

    audio_reactive: bool
    author: str
    category: str
    description: str
    id: str
    name: str
    runnable: bool
    source: str
    tags: list[str]
    version: str
    cover_image_url: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        audio_reactive = self.audio_reactive

        author = self.author

        category = self.category

        description = self.description

        id = self.id

        name = self.name

        runnable = self.runnable

        source = self.source

        tags = self.tags

        version = self.version

        cover_image_url: None | str | Unset
        if isinstance(self.cover_image_url, Unset):
            cover_image_url = UNSET
        else:
            cover_image_url = self.cover_image_url

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "audio_reactive": audio_reactive,
                "author": author,
                "category": category,
                "description": description,
                "id": id,
                "name": name,
                "runnable": runnable,
                "source": source,
                "tags": tags,
                "version": version,
            }
        )
        if cover_image_url is not UNSET:
            field_dict["cover_image_url"] = cover_image_url

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        audio_reactive = d.pop("audio_reactive")

        author = d.pop("author")

        category = d.pop("category")

        description = d.pop("description")

        id = d.pop("id")

        name = d.pop("name")

        runnable = d.pop("runnable")

        source = d.pop("source")

        tags = cast(list[str], d.pop("tags"))

        version = d.pop("version")

        def _parse_cover_image_url(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        cover_image_url = _parse_cover_image_url(d.pop("cover_image_url", UNSET))

        effect_summary = cls(
            audio_reactive=audio_reactive,
            author=author,
            category=category,
            description=description,
            id=id,
            name=name,
            runnable=runnable,
            source=source,
            tags=tags,
            version=version,
            cover_image_url=cover_image_url,
        )

        effect_summary.additional_properties = d
        return effect_summary

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
