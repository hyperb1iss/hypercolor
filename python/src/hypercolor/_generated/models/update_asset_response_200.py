from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.media_asset_record import MediaAssetRecord
    from ..models.response_meta import ResponseMeta


T = TypeVar("T", bound="UpdateAssetResponse200")


@_attrs_define
class UpdateAssetResponse200:
    """
    Attributes:
        data (MediaAssetRecord): Persisted metadata for one user media asset.
        meta (ResponseMeta): Response metadata included in every envelope.
    """

    data: MediaAssetRecord
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
        from ..models.media_asset_record import MediaAssetRecord
        from ..models.response_meta import ResponseMeta

        d = dict(src_dict)
        data = MediaAssetRecord.from_dict(d.pop("data"))

        meta = ResponseMeta.from_dict(d.pop("meta"))

        update_asset_response_200 = cls(
            data=data,
            meta=meta,
        )

        update_asset_response_200.additional_properties = d
        return update_asset_response_200

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
