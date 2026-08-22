from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.response_meta import ResponseMeta
    from ..models.zone_resource import ZoneResource


T = TypeVar("T", bound="PatchLiveLayerControlsResponse200")


@_attrs_define
class PatchLiveLayerControlsResponse200:
    """
    Attributes:
        data (ZoneResource): One authored zone inside the live document (Spec 78 §1.3).
        meta (ResponseMeta): Response metadata included in every envelope.
    """

    data: ZoneResource
    meta: ResponseMeta
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        data = self.data.to_dict()

        meta = self.meta.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "data": data,
                "meta": meta,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.response_meta import ResponseMeta
        from ..models.zone_resource import ZoneResource

        d = dict(src_dict)
        data = ZoneResource.from_dict(d.pop("data"))

        meta = ResponseMeta.from_dict(d.pop("meta"))

        patch_live_layer_controls_response_200 = cls(
            data=data,
            meta=meta,
        )

        patch_live_layer_controls_response_200.additional_properties = d
        return patch_live_layer_controls_response_200

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
