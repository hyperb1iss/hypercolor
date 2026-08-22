from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define

from ..models.blend_mode import BlendMode
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.layer_adjust import LayerAdjust
    from ..models.layer_binding import LayerBinding
    from ..models.layer_source import LayerSource
    from ..models.layer_transform import LayerTransform


T = TypeVar("T", bound="CreateLayerRequest")


@_attrs_define
class CreateLayerRequest:
    """`POST /scene/zones/{zone}/layers`: append a layer to the stack.

    The server mints the layer id (Spec 78 §1.4); the response's zone
    resource carries it.

        Attributes:
            source (LayerSource): Source that feeds one authored layer.
            adjust (LayerAdjust | None | Unset):
            bindings (list[LayerBinding] | None | Unset):
            blend (BlendMode | None | Unset):
            enabled (bool | None | Unset):
            name (None | str | Unset):
            opacity (float | None | Unset):
            transform (LayerTransform | None | Unset):
    """

    source: LayerSource
    adjust: LayerAdjust | None | Unset = UNSET
    bindings: list[LayerBinding] | None | Unset = UNSET
    blend: BlendMode | None | Unset = UNSET
    enabled: bool | None | Unset = UNSET
    name: None | str | Unset = UNSET
    opacity: float | None | Unset = UNSET
    transform: LayerTransform | None | Unset = UNSET

    def to_dict(self) -> dict[str, Any]:
        from ..models.layer_adjust import LayerAdjust
        from ..models.layer_transform import LayerTransform

        source = self.source.to_dict()

        adjust: dict[str, Any] | None | Unset
        if isinstance(self.adjust, Unset):
            adjust = UNSET
        elif isinstance(self.adjust, LayerAdjust):
            adjust = self.adjust.to_dict()
        else:
            adjust = self.adjust

        bindings: list[dict[str, Any]] | None | Unset
        if isinstance(self.bindings, Unset):
            bindings = UNSET
        elif isinstance(self.bindings, list):
            bindings = []
            for bindings_type_0_item_data in self.bindings:
                bindings_type_0_item = bindings_type_0_item_data.to_dict()
                bindings.append(bindings_type_0_item)

        else:
            bindings = self.bindings

        blend: None | str | Unset
        if isinstance(self.blend, Unset):
            blend = UNSET
        elif isinstance(self.blend, BlendMode):
            blend = self.blend.value
        else:
            blend = self.blend

        enabled: bool | None | Unset
        if isinstance(self.enabled, Unset):
            enabled = UNSET
        else:
            enabled = self.enabled

        name: None | str | Unset
        if isinstance(self.name, Unset):
            name = UNSET
        else:
            name = self.name

        opacity: float | None | Unset
        if isinstance(self.opacity, Unset):
            opacity = UNSET
        else:
            opacity = self.opacity

        transform: dict[str, Any] | None | Unset
        if isinstance(self.transform, Unset):
            transform = UNSET
        elif isinstance(self.transform, LayerTransform):
            transform = self.transform.to_dict()
        else:
            transform = self.transform

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
        source = LayerSource.from_dict(d.pop("source"))

        def _parse_adjust(data: object) -> LayerAdjust | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                adjust_type_1 = LayerAdjust.from_dict(data)

                return adjust_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(LayerAdjust | None | Unset, data)

        adjust = _parse_adjust(d.pop("adjust", UNSET))

        def _parse_bindings(data: object) -> list[LayerBinding] | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, list):
                    raise TypeError()
                bindings_type_0 = []
                _bindings_type_0 = data
                for bindings_type_0_item_data in _bindings_type_0:
                    bindings_type_0_item = LayerBinding.from_dict(
                        bindings_type_0_item_data
                    )

                    bindings_type_0.append(bindings_type_0_item)

                return bindings_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(list[LayerBinding] | None | Unset, data)

        bindings = _parse_bindings(d.pop("bindings", UNSET))

        def _parse_blend(data: object) -> BlendMode | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                blend_type_1 = BlendMode(data)

                return blend_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(BlendMode | None | Unset, data)

        blend = _parse_blend(d.pop("blend", UNSET))

        def _parse_enabled(data: object) -> bool | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(bool | None | Unset, data)

        enabled = _parse_enabled(d.pop("enabled", UNSET))

        def _parse_name(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        name = _parse_name(d.pop("name", UNSET))

        def _parse_opacity(data: object) -> float | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(float | None | Unset, data)

        opacity = _parse_opacity(d.pop("opacity", UNSET))

        def _parse_transform(data: object) -> LayerTransform | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                transform_type_1 = LayerTransform.from_dict(data)

                return transform_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(LayerTransform | None | Unset, data)

        transform = _parse_transform(d.pop("transform", UNSET))

        create_layer_request = cls(
            source=source,
            adjust=adjust,
            bindings=bindings,
            blend=blend,
            enabled=enabled,
            name=name,
            opacity=opacity,
            transform=transform,
        )

        return create_layer_request
