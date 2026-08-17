from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.control_definition import ControlDefinition
    from ..models.effect_capability_set import EffectCapabilitySet
    from ..models.preset_template import PresetTemplate


T = TypeVar("T", bound="EffectSummary")


@_attrs_define
class EffectSummary:
    """One effect in the list response.

    `controls` and `presets` are expansions: they are absent unless the
    request asked for them via `include=controls,presets`, so the default
    list shape is unchanged and a client that ignores the parameter sees
    exactly the payload it saw before.

        Attributes:
            author (str):
            category (str):
            description (str):
            id (str):
            name (str):
            runnable (bool):
            source (str):
            tags (list[str]):
            version (str):
            audio_reactive (bool | Unset):
            capabilities (EffectCapabilitySet | Unset): Typed source requirements declared by an effect.
            controls (list[ControlDefinition] | None | Unset):
            cover_image_url (None | str | Unset):
            input_reactive (bool | Unset):
            presets (list[PresetTemplate] | None | Unset):
    """

    author: str
    category: str
    description: str
    id: str
    name: str
    runnable: bool
    source: str
    tags: list[str]
    version: str
    audio_reactive: bool | Unset = UNSET
    capabilities: EffectCapabilitySet | Unset = UNSET
    controls: list[ControlDefinition] | None | Unset = UNSET
    cover_image_url: None | str | Unset = UNSET
    input_reactive: bool | Unset = UNSET
    presets: list[PresetTemplate] | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        author = self.author

        category = self.category

        description = self.description

        id = self.id

        name = self.name

        runnable = self.runnable

        source = self.source

        tags = self.tags

        version = self.version

        audio_reactive = self.audio_reactive

        capabilities: dict[str, Any] | Unset = UNSET
        if not isinstance(self.capabilities, Unset):
            capabilities = self.capabilities.to_dict()

        controls: list[dict[str, Any]] | None | Unset
        if isinstance(self.controls, Unset):
            controls = UNSET
        elif isinstance(self.controls, list):
            controls = []
            for controls_type_0_item_data in self.controls:
                controls_type_0_item = controls_type_0_item_data.to_dict()
                controls.append(controls_type_0_item)

        else:
            controls = self.controls

        cover_image_url: None | str | Unset
        if isinstance(self.cover_image_url, Unset):
            cover_image_url = UNSET
        else:
            cover_image_url = self.cover_image_url

        input_reactive = self.input_reactive

        presets: list[dict[str, Any]] | None | Unset
        if isinstance(self.presets, Unset):
            presets = UNSET
        elif isinstance(self.presets, list):
            presets = []
            for presets_type_0_item_data in self.presets:
                presets_type_0_item = presets_type_0_item_data.to_dict()
                presets.append(presets_type_0_item)

        else:
            presets = self.presets

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
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
        if audio_reactive is not UNSET:
            field_dict["audio_reactive"] = audio_reactive
        if capabilities is not UNSET:
            field_dict["capabilities"] = capabilities
        if controls is not UNSET:
            field_dict["controls"] = controls
        if cover_image_url is not UNSET:
            field_dict["cover_image_url"] = cover_image_url
        if input_reactive is not UNSET:
            field_dict["input_reactive"] = input_reactive
        if presets is not UNSET:
            field_dict["presets"] = presets

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.control_definition import ControlDefinition
        from ..models.effect_capability_set import EffectCapabilitySet
        from ..models.preset_template import PresetTemplate

        d = dict(src_dict)
        author = d.pop("author")

        category = d.pop("category")

        description = d.pop("description")

        id = d.pop("id")

        name = d.pop("name")

        runnable = d.pop("runnable")

        source = d.pop("source")

        tags = cast(list[str], d.pop("tags"))

        version = d.pop("version")

        audio_reactive = d.pop("audio_reactive", UNSET)

        _capabilities = d.pop("capabilities", UNSET)
        capabilities: EffectCapabilitySet | Unset
        if isinstance(_capabilities, Unset):
            capabilities = UNSET
        else:
            capabilities = EffectCapabilitySet.from_dict(_capabilities)

        def _parse_controls(data: object) -> list[ControlDefinition] | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, list):
                    raise TypeError()
                controls_type_0 = []
                _controls_type_0 = data
                for controls_type_0_item_data in _controls_type_0:
                    controls_type_0_item = ControlDefinition.from_dict(
                        controls_type_0_item_data
                    )

                    controls_type_0.append(controls_type_0_item)

                return controls_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(list[ControlDefinition] | None | Unset, data)

        controls = _parse_controls(d.pop("controls", UNSET))

        def _parse_cover_image_url(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        cover_image_url = _parse_cover_image_url(d.pop("cover_image_url", UNSET))

        input_reactive = d.pop("input_reactive", UNSET)

        def _parse_presets(data: object) -> list[PresetTemplate] | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, list):
                    raise TypeError()
                presets_type_0 = []
                _presets_type_0 = data
                for presets_type_0_item_data in _presets_type_0:
                    presets_type_0_item = PresetTemplate.from_dict(
                        presets_type_0_item_data
                    )

                    presets_type_0.append(presets_type_0_item)

                return presets_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(list[PresetTemplate] | None | Unset, data)

        presets = _parse_presets(d.pop("presets", UNSET))

        effect_summary = cls(
            author=author,
            category=category,
            description=description,
            id=id,
            name=name,
            runnable=runnable,
            source=source,
            tags=tags,
            version=version,
            audio_reactive=audio_reactive,
            capabilities=capabilities,
            controls=controls,
            cover_image_url=cover_image_url,
            input_reactive=input_reactive,
            presets=presets,
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
