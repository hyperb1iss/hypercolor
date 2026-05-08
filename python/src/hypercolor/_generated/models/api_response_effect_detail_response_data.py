from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.api_response_effect_detail_response_data_active_control_values_type_0 import (
        ApiResponseEffectDetailResponseDataActiveControlValuesType0,
    )
    from ..models.control_definition import ControlDefinition
    from ..models.preset_template import PresetTemplate


T = TypeVar("T", bound="ApiResponseEffectDetailResponseData")


@_attrs_define
class ApiResponseEffectDetailResponseData:
    """
    Attributes:
        audio_reactive (bool):
        author (str):
        category (str):
        controls (list[ControlDefinition]):
        description (str):
        id (str):
        name (str):
        runnable (bool):
        source (str):
        tags (list[str]):
        version (str):
        active_control_values (ApiResponseEffectDetailResponseDataActiveControlValuesType0 | None | Unset):
        cover_image_url (None | str | Unset):
        presets (list[PresetTemplate] | Unset):
    """

    audio_reactive: bool
    author: str
    category: str
    controls: list[ControlDefinition]
    description: str
    id: str
    name: str
    runnable: bool
    source: str
    tags: list[str]
    version: str
    active_control_values: (
        ApiResponseEffectDetailResponseDataActiveControlValuesType0 | None | Unset
    ) = UNSET
    cover_image_url: None | str | Unset = UNSET
    presets: list[PresetTemplate] | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.api_response_effect_detail_response_data_active_control_values_type_0 import (
            ApiResponseEffectDetailResponseDataActiveControlValuesType0,
        )

        audio_reactive = self.audio_reactive

        author = self.author

        category = self.category

        controls = []
        for controls_item_data in self.controls:
            controls_item = controls_item_data.to_dict()
            controls.append(controls_item)

        description = self.description

        id = self.id

        name = self.name

        runnable = self.runnable

        source = self.source

        tags = self.tags

        version = self.version

        active_control_values: dict[str, Any] | None | Unset
        if isinstance(self.active_control_values, Unset):
            active_control_values = UNSET
        elif isinstance(
            self.active_control_values,
            ApiResponseEffectDetailResponseDataActiveControlValuesType0,
        ):
            active_control_values = self.active_control_values.to_dict()
        else:
            active_control_values = self.active_control_values

        cover_image_url: None | str | Unset
        if isinstance(self.cover_image_url, Unset):
            cover_image_url = UNSET
        else:
            cover_image_url = self.cover_image_url

        presets: list[dict[str, Any]] | Unset = UNSET
        if not isinstance(self.presets, Unset):
            presets = []
            for presets_item_data in self.presets:
                presets_item = presets_item_data.to_dict()
                presets.append(presets_item)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "audio_reactive": audio_reactive,
                "author": author,
                "category": category,
                "controls": controls,
                "description": description,
                "id": id,
                "name": name,
                "runnable": runnable,
                "source": source,
                "tags": tags,
                "version": version,
            }
        )
        if active_control_values is not UNSET:
            field_dict["active_control_values"] = active_control_values
        if cover_image_url is not UNSET:
            field_dict["cover_image_url"] = cover_image_url
        if presets is not UNSET:
            field_dict["presets"] = presets

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.api_response_effect_detail_response_data_active_control_values_type_0 import (
            ApiResponseEffectDetailResponseDataActiveControlValuesType0,
        )
        from ..models.control_definition import ControlDefinition
        from ..models.preset_template import PresetTemplate

        d = dict(src_dict)
        audio_reactive = d.pop("audio_reactive")

        author = d.pop("author")

        category = d.pop("category")

        controls = []
        _controls = d.pop("controls")
        for controls_item_data in _controls:
            controls_item = ControlDefinition.from_dict(controls_item_data)

            controls.append(controls_item)

        description = d.pop("description")

        id = d.pop("id")

        name = d.pop("name")

        runnable = d.pop("runnable")

        source = d.pop("source")

        tags = cast(list[str], d.pop("tags"))

        version = d.pop("version")

        def _parse_active_control_values(
            data: object,
        ) -> ApiResponseEffectDetailResponseDataActiveControlValuesType0 | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                active_control_values_type_0 = ApiResponseEffectDetailResponseDataActiveControlValuesType0.from_dict(
                    data
                )

                return active_control_values_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(
                ApiResponseEffectDetailResponseDataActiveControlValuesType0
                | None
                | Unset,
                data,
            )

        active_control_values = _parse_active_control_values(
            d.pop("active_control_values", UNSET)
        )

        def _parse_cover_image_url(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        cover_image_url = _parse_cover_image_url(d.pop("cover_image_url", UNSET))

        _presets = d.pop("presets", UNSET)
        presets: list[PresetTemplate] | Unset = UNSET
        if _presets is not UNSET:
            presets = []
            for presets_item_data in _presets:
                presets_item = PresetTemplate.from_dict(presets_item_data)

                presets.append(presets_item)

        api_response_effect_detail_response_data = cls(
            audio_reactive=audio_reactive,
            author=author,
            category=category,
            controls=controls,
            description=description,
            id=id,
            name=name,
            runnable=runnable,
            source=source,
            tags=tags,
            version=version,
            active_control_values=active_control_values,
            cover_image_url=cover_image_url,
            presets=presets,
        )

        api_response_effect_detail_response_data.additional_properties = d
        return api_response_effect_detail_response_data

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
