from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.layer_parameter import LayerParameter

if TYPE_CHECKING:
    from ..models.binding_map import BindingMap
    from ..models.binding_source_type_0 import BindingSourceType0
    from ..models.binding_source_type_1 import BindingSourceType1
    from ..models.binding_source_type_2 import BindingSourceType2
    from ..models.binding_source_type_3 import BindingSourceType3


T = TypeVar("T", bound="LayerBinding")


@_attrs_define
class LayerBinding:
    """Live mapping from runtime data to a scalar layer parameter.

    Attributes:
        map_ (BindingMap): Linear mapping from source values into target parameter values.
        source (BindingSourceType0 | BindingSourceType1 | BindingSourceType2 | BindingSourceType3): Runtime source that
            drives a layer binding.
        target (LayerParameter): Bindable scalar layer parameters.
    """

    map_: BindingMap
    source: (
        BindingSourceType0
        | BindingSourceType1
        | BindingSourceType2
        | BindingSourceType3
    )
    target: LayerParameter
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.binding_source_type_0 import BindingSourceType0
        from ..models.binding_source_type_1 import BindingSourceType1
        from ..models.binding_source_type_2 import BindingSourceType2

        map_ = self.map_.to_dict()

        source: dict[str, Any]
        if isinstance(self.source, BindingSourceType0):
            source = self.source.to_dict()
        elif isinstance(self.source, BindingSourceType1):
            source = self.source.to_dict()
        elif isinstance(self.source, BindingSourceType2):
            source = self.source.to_dict()
        else:
            source = self.source.to_dict()

        target = self.target.value

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "map": map_,
                "source": source,
                "target": target,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.binding_map import BindingMap
        from ..models.binding_source_type_0 import BindingSourceType0
        from ..models.binding_source_type_1 import BindingSourceType1
        from ..models.binding_source_type_2 import BindingSourceType2
        from ..models.binding_source_type_3 import BindingSourceType3

        d = dict(src_dict)
        map_ = BindingMap.from_dict(d.pop("map"))

        def _parse_source(
            data: object,
        ) -> (
            BindingSourceType0
            | BindingSourceType1
            | BindingSourceType2
            | BindingSourceType3
        ):
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_binding_source_type_0 = BindingSourceType0.from_dict(
                    data
                )

                return componentsschemas_binding_source_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_binding_source_type_1 = BindingSourceType1.from_dict(
                    data
                )

                return componentsschemas_binding_source_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_binding_source_type_2 = BindingSourceType2.from_dict(
                    data
                )

                return componentsschemas_binding_source_type_2
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            if not isinstance(data, dict):
                raise TypeError()
            componentsschemas_binding_source_type_3 = BindingSourceType3.from_dict(data)

            return componentsschemas_binding_source_type_3

        source = _parse_source(d.pop("source"))

        target = LayerParameter(d.pop("target"))

        layer_binding = cls(
            map_=map_,
            source=source,
            target=target,
        )

        layer_binding.additional_properties = d
        return layer_binding

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
