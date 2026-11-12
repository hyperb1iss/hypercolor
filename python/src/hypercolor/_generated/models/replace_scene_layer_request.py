from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast
from uuid import UUID

from attrs import define as _attrs_define
from typing_extensions import Self

from ..models.blend_mode import BlendMode
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.layer_adjust import LayerAdjust
    from ..models.layer_binding import LayerBinding
    from ..models.layer_scope import LayerScope
    from ..models.layer_source import LayerSource
    from ..models.layer_transform import LayerTransform


T = TypeVar("T", bound="ReplaceSceneLayerRequest")


@_attrs_define
class ReplaceSceneLayerRequest:
    """One layer of a zone in a whole-scene replacement.

    A layer that carries an `id` replaces the stored layer with that id;
    one without is new. `scope` has three states: leaving it out keeps the
    stored layer's scope, `null` applies the layer to its whole zone, and
    a value sets it.

        Attributes:
            source (LayerSource): Source that feeds one authored layer.
            adjust (LayerAdjust | Unset): Per-layer color adjustment settings.
            bindings (list[LayerBinding] | Unset):
            blend (BlendMode | Unset): Blend mode used by authored layers and display faces.
            enabled (bool | Unset):
            id (None | Unset | UUID):
            name (None | str | Unset):
            opacity (float | Unset):
            scope (LayerScope | None | Unset):
            transform (LayerTransform | Unset): Geometric placement for a layer source.
    """

    source: LayerSource
    adjust: LayerAdjust | Unset = UNSET
    bindings: list[LayerBinding] | Unset = UNSET
    blend: BlendMode | Unset = UNSET
    enabled: bool | Unset = UNSET
    id: None | Unset | UUID = UNSET
    name: None | str | Unset = UNSET
    opacity: float | Unset = UNSET
    scope: LayerScope | None | Unset = UNSET
    transform: LayerTransform | Unset = UNSET

    def to_dict(self) -> dict[str, Any]:
        from ..models.layer_scope import LayerScope

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

        id: None | str | Unset
        if isinstance(self.id, Unset):
            id = UNSET
        elif isinstance(self.id, UUID):
            id = str(self.id)
        else:
            id = self.id

        name: None | str | Unset
        if isinstance(self.name, Unset):
            name = UNSET
        else:
            name = self.name

        opacity = self.opacity

        scope: dict[str, Any] | None | Unset
        if isinstance(self.scope, Unset):
            scope = UNSET
        elif isinstance(self.scope, LayerScope):
            scope = self.scope.to_dict()
        else:
            scope = self.scope

        transform: dict[str, Any] | Unset = UNSET
        if not isinstance(self.transform, Unset):
            transform = self.transform.to_dict()

        field_dict: dict[str, Any] = {}

        field_dict.update(
            {
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
        if id is not UNSET:
            field_dict["id"] = id
        if name is not UNSET:
            field_dict["name"] = name
        if opacity is not UNSET:
            field_dict["opacity"] = opacity
        if scope is not UNSET:
            field_dict["scope"] = scope
        if transform is not UNSET:
            field_dict["transform"] = transform

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.layer_adjust import LayerAdjust
        from ..models.layer_binding import LayerBinding
        from ..models.layer_scope import LayerScope
        from ..models.layer_source import LayerSource
        from ..models.layer_transform import LayerTransform

        d = dict(src_dict)
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
        blend: BlendMode | Unset
        if isinstance(_blend, Unset):
            blend = UNSET
        else:
            blend = BlendMode(_blend)

        enabled = d.pop("enabled", UNSET)

        def _parse_id(data: object) -> None | Unset | UUID:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                id_type_0 = UUID(data)

                return id_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | Unset | UUID, data)

        id = _parse_id(d.pop("id", UNSET))

        def _parse_name(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        name = _parse_name(d.pop("name", UNSET))

        opacity = d.pop("opacity", UNSET)

        def _parse_scope(data: object) -> LayerScope | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                scope_type_0 = LayerScope.from_dict(data)

                return scope_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(LayerScope | None | Unset, data)

        scope = _parse_scope(d.pop("scope", UNSET))

        _transform = d.pop("transform", UNSET)
        transform: LayerTransform | Unset
        if isinstance(_transform, Unset):
            transform = UNSET
        else:
            transform = LayerTransform.from_dict(_transform)

        replace_scene_layer_request = cls(
            source=source,
            adjust=adjust,
            bindings=bindings,
            blend=blend,
            enabled=enabled,
            id=id,
            name=name,
            opacity=opacity,
            scope=scope,
            transform=transform,
        )

        return replace_scene_layer_request
