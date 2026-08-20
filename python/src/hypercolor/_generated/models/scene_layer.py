from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast
from uuid import UUID

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.layer_blend_mode import LayerBlendMode
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.layer_adjust import LayerAdjust
    from ..models.layer_binding import LayerBinding
    from ..models.layer_source import LayerSource
    from ..models.layer_transform import LayerTransform


T = TypeVar("T", bound="SceneLayer")


@_attrs_define
class SceneLayer:
    """Authored layer inside a zone's bottom-to-top stack.

    Attributes:
        id (UUID): Stable identifier for a layer within a zone.
        source (LayerSource): Source that feeds one authored layer.
        adjust (LayerAdjust | Unset): Per-layer color adjustment settings.
        bindings (list[LayerBinding] | Unset): Live scalar bindings for layer parameters.
        blend (LayerBlendMode | Unset): Layer blend mode used by authored stacks.
        enabled (bool | Unset): Whether this layer is currently active.
        name (None | str | Unset): Display name. Defaults to the source's intrinsic name in the UI.
        opacity (float | Unset): Layer opacity.
        transform (LayerTransform | Unset): Geometric placement for a layer source.
    """

    id: UUID
    source: LayerSource
    adjust: LayerAdjust | Unset = UNSET
    bindings: list[LayerBinding] | Unset = UNSET
    blend: LayerBlendMode | Unset = UNSET
    enabled: bool | Unset = UNSET
    name: None | str | Unset = UNSET
    opacity: float | Unset = UNSET
    transform: LayerTransform | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        id = str(self.id)

        source = self.source.to_dict()

        adjust: dict[str, Any] | Unset = UNSET
        if not isinstance(self.adjust, Unset):
            adjust = self.adjust.to_dict()

        bindings: list[dict[str, Any]] | Unset = UNSET
        if not isinstance(self.bindings, Unset):
            bindings = []
            for bindings_item_data in self.bindings:
                bindings_item = bindings_item_data.to_dict()
                bindings.append(bindings_item)

        blend: str | Unset = UNSET
        if not isinstance(self.blend, Unset):
            blend = self.blend.value

        enabled = self.enabled

        name: None | str | Unset
        if isinstance(self.name, Unset):
            name = UNSET
        else:
            name = self.name

        opacity = self.opacity

        transform: dict[str, Any] | Unset = UNSET
        if not isinstance(self.transform, Unset):
            transform = self.transform.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "id": id,
                "source": source,
            }
        )
        if adjust is not UNSET:
            field_dict["adjust"] = adjust
        if bindings is not UNSET:
            field_dict["bindings"] = bindings
        if blend is not UNSET:
            field_dict["blend"] = blend
        if enabled is not UNSET:
            field_dict["enabled"] = enabled
        if name is not UNSET:
            field_dict["name"] = name
        if opacity is not UNSET:
            field_dict["opacity"] = opacity
        if transform is not UNSET:
            field_dict["transform"] = transform

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.layer_adjust import LayerAdjust
        from ..models.layer_binding import LayerBinding
        from ..models.layer_source import LayerSource
        from ..models.layer_transform import LayerTransform

        d = dict(src_dict)
        id = UUID(d.pop("id"))

        source = LayerSource.from_dict(d.pop("source"))

        _adjust = d.pop("adjust", UNSET)
        adjust: LayerAdjust | Unset
        if isinstance(_adjust, Unset):
            adjust = UNSET
        else:
            adjust = LayerAdjust.from_dict(_adjust)

        _bindings = d.pop("bindings", UNSET)
        bindings: list[LayerBinding] | Unset = UNSET
        if _bindings is not UNSET:
            bindings = []
            for bindings_item_data in _bindings:
                bindings_item = LayerBinding.from_dict(bindings_item_data)

                bindings.append(bindings_item)

        _blend = d.pop("blend", UNSET)
        blend: LayerBlendMode | Unset
        if isinstance(_blend, Unset):
            blend = UNSET
        else:
            blend = LayerBlendMode(_blend)

        enabled = d.pop("enabled", UNSET)

        def _parse_name(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        name = _parse_name(d.pop("name", UNSET))

        opacity = d.pop("opacity", UNSET)

        _transform = d.pop("transform", UNSET)
        transform: LayerTransform | Unset
        if isinstance(_transform, Unset):
            transform = UNSET
        else:
            transform = LayerTransform.from_dict(_transform)

        scene_layer = cls(
            id=id,
            source=source,
            adjust=adjust,
            bindings=bindings,
            blend=blend,
            enabled=enabled,
            name=name,
            opacity=opacity,
            transform=transform,
        )

        scene_layer.additional_properties = d
        return scene_layer

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
