from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.create_layer_request_adjust import CreateLayerRequestAdjust
    from ..models.create_layer_request_bindings_item import (
        CreateLayerRequestBindingsItem,
    )
    from ..models.create_layer_request_source import CreateLayerRequestSource
    from ..models.create_layer_request_transform import CreateLayerRequestTransform


T = TypeVar("T", bound="CreateLayerRequest")


@_attrs_define
class CreateLayerRequest:
    """
    Attributes:
        source (CreateLayerRequestSource):
        adjust (CreateLayerRequestAdjust | Unset):
        bindings (list[CreateLayerRequestBindingsItem] | Unset):
        blend (str | Unset):
        enabled (bool | Unset):
        name (None | str | Unset):
        opacity (float | Unset):
        transform (CreateLayerRequestTransform | Unset):
    """

    source: CreateLayerRequestSource
    adjust: CreateLayerRequestAdjust | Unset = UNSET
    bindings: list[CreateLayerRequestBindingsItem] | Unset = UNSET
    blend: str | Unset = UNSET
    enabled: bool | Unset = UNSET
    name: None | str | Unset = UNSET
    opacity: float | Unset = UNSET
    transform: CreateLayerRequestTransform | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
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

        blend = self.blend

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
        from ..models.create_layer_request_adjust import CreateLayerRequestAdjust
        from ..models.create_layer_request_bindings_item import (
            CreateLayerRequestBindingsItem,
        )
        from ..models.create_layer_request_source import CreateLayerRequestSource
        from ..models.create_layer_request_transform import CreateLayerRequestTransform

        d = dict(src_dict)
        source = CreateLayerRequestSource.from_dict(d.pop("source"))

        _adjust = d.pop("adjust", UNSET)
        adjust: CreateLayerRequestAdjust | Unset
        if isinstance(_adjust, Unset):
            adjust = UNSET
        else:
            adjust = CreateLayerRequestAdjust.from_dict(_adjust)

        _bindings = d.pop("bindings", UNSET)
        bindings: list[CreateLayerRequestBindingsItem] | Unset = UNSET
        if _bindings is not UNSET:
            bindings = []
            for bindings_item_data in _bindings:
                bindings_item = CreateLayerRequestBindingsItem.from_dict(
                    bindings_item_data
                )

                bindings.append(bindings_item)

        blend = d.pop("blend", UNSET)

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
        transform: CreateLayerRequestTransform | Unset
        if isinstance(_transform, Unset):
            transform = UNSET
        else:
            transform = CreateLayerRequestTransform.from_dict(_transform)

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

        create_layer_request.additional_properties = d
        return create_layer_request

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
