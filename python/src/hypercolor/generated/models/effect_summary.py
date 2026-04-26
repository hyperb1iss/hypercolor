from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

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
