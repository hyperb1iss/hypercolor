from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast
from uuid import UUID

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.effect_category import EffectCategory
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.control_definition import ControlDefinition
    from ..models.effect_source import EffectSource
    from ..models.preset_template import PresetTemplate


T = TypeVar("T", bound="EffectMetadata")


@_attrs_define
class EffectMetadata:
    """Universal effect descriptor.

    Serialized as TOML for native effects and as JSON for the REST API
    and WebSocket protocol. This is the canonical metadata attached to
    every effect regardless of rendering path.

        Attributes:
            author (str): Author or publisher name.
            description (str): Short description (max 200 chars). Shown in the effect browser.
            id (UUID): Unique identifier for an effect, wrapping a UUID v7.

                Generated at discovery time and used as the primary key across
                the registry, event bus, API, and UI.
            name (str): Human-readable display name.
            source (EffectSource): Identifies the rendering path and source location for an effect.

                Determines which renderer handles the effect (wgpu vs. Servo).
            audio_reactive (bool | Unset): Indicates whether the effect expects audio payload injection.
            category (EffectCategory | Unset): Primary classification categories for the effect taxonomy.

                An effect can belong to multiple categories. Used for discovery
                and filtering in the effect browser UI.
            controls (list[ControlDefinition] | Unset): User-facing controls declared by this effect.
            input_reactive (bool | Unset): Indicates whether the effect expects host input payload injection.
            license_ (None | str | Unset): SPDX license identifier (e.g. `"MIT"`, `"Apache-2.0"`).
            presets (list[PresetTemplate] | Unset): Effect-defined preset snapshots. Authored by the effect developer,
                read-only at runtime. Shown alongside user-created presets in the UI.
            screen_reactive (bool | Unset): Indicates whether the effect expects screen capture payload injection.
            tags (list[str] | Unset): Discovery and taxonomy tags. Free-form, lowercase, hyphenated.
            version (str | Unset): Semantic version string (e.g. `"1.2.0"`).
    """

    author: str
    description: str
    id: UUID
    name: str
    source: EffectSource
    audio_reactive: bool | Unset = UNSET
    category: EffectCategory | Unset = UNSET
    controls: list[ControlDefinition] | Unset = UNSET
    input_reactive: bool | Unset = UNSET
    license_: None | str | Unset = UNSET
    presets: list[PresetTemplate] | Unset = UNSET
    screen_reactive: bool | Unset = UNSET
    tags: list[str] | Unset = UNSET
    version: str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        author = self.author

        description = self.description

        id = str(self.id)

        name = self.name

        source = self.source.to_dict()

        audio_reactive = self.audio_reactive

        category: str | Unset = UNSET
        if not isinstance(self.category, Unset):
            category = self.category.value

        controls: list[dict[str, Any]] | Unset = UNSET
        if not isinstance(self.controls, Unset):
            controls = []
            for controls_item_data in self.controls:
                controls_item = controls_item_data.to_dict()
                controls.append(controls_item)

        input_reactive = self.input_reactive

        license_: None | str | Unset
        if isinstance(self.license_, Unset):
            license_ = UNSET
        else:
            license_ = self.license_

        presets: list[dict[str, Any]] | Unset = UNSET
        if not isinstance(self.presets, Unset):
            presets = []
            for presets_item_data in self.presets:
                presets_item = presets_item_data.to_dict()
                presets.append(presets_item)

        screen_reactive = self.screen_reactive

        tags: list[str] | Unset = UNSET
        if not isinstance(self.tags, Unset):
            tags = self.tags

        version = self.version

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "author": author,
                "description": description,
                "id": id,
                "name": name,
                "source": source,
            }
        )
        if audio_reactive is not UNSET:
            field_dict["audio_reactive"] = audio_reactive
        if category is not UNSET:
            field_dict["category"] = category
        if controls is not UNSET:
            field_dict["controls"] = controls
        if input_reactive is not UNSET:
            field_dict["input_reactive"] = input_reactive
        if license_ is not UNSET:
            field_dict["license"] = license_
        if presets is not UNSET:
            field_dict["presets"] = presets
        if screen_reactive is not UNSET:
            field_dict["screen_reactive"] = screen_reactive
        if tags is not UNSET:
            field_dict["tags"] = tags
        if version is not UNSET:
            field_dict["version"] = version

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.control_definition import ControlDefinition
        from ..models.effect_source import EffectSource
        from ..models.preset_template import PresetTemplate

        d = dict(src_dict)
        author = d.pop("author")

        description = d.pop("description")

        id = UUID(d.pop("id"))

        name = d.pop("name")

        source = EffectSource.from_dict(d.pop("source"))

        audio_reactive = d.pop("audio_reactive", UNSET)

        _category = d.pop("category", UNSET)
        category: EffectCategory | Unset
        if isinstance(_category, Unset):
            category = UNSET
        else:
            category = EffectCategory(_category)

        _controls = d.pop("controls", UNSET)
        controls: list[ControlDefinition] | Unset = UNSET
        if _controls is not UNSET:
            controls = []
            for controls_item_data in _controls:
                controls_item = ControlDefinition.from_dict(controls_item_data)

                controls.append(controls_item)

        input_reactive = d.pop("input_reactive", UNSET)

        def _parse_license_(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        license_ = _parse_license_(d.pop("license", UNSET))

        _presets = d.pop("presets", UNSET)
        presets: list[PresetTemplate] | Unset = UNSET
        if _presets is not UNSET:
            presets = []
            for presets_item_data in _presets:
                presets_item = PresetTemplate.from_dict(presets_item_data)

                presets.append(presets_item)

        screen_reactive = d.pop("screen_reactive", UNSET)

        tags = cast(list[str], d.pop("tags", UNSET))

        version = d.pop("version", UNSET)

        effect_metadata = cls(
            author=author,
            description=description,
            id=id,
            name=name,
            source=source,
            audio_reactive=audio_reactive,
            category=category,
            controls=controls,
            input_reactive=input_reactive,
            license_=license_,
            presets=presets,
            screen_reactive=screen_reactive,
            tags=tags,
            version=version,
        )

        effect_metadata.additional_properties = d
        return effect_metadata

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
