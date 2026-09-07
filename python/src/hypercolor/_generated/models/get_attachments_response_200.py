from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.device_components_response import DeviceComponentsResponse
    from ..models.response_meta import ResponseMeta


T = TypeVar("T", bound="GetAttachmentsResponse200")


@_attrs_define
class GetAttachmentsResponse200:
    """
    Attributes:
        data (DeviceComponentsResponse): Response for `GET /api/v1/devices/{id}/attachments`.

            `slots` are the controller's physical attachment points, `bindings`
            what is attached to them, and `suggested_zones` the layout zones the
            attachments imply.
        meta (ResponseMeta): Response metadata included in every envelope.
    """

    data: DeviceComponentsResponse
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
        from ..models.device_components_response import DeviceComponentsResponse
        from ..models.response_meta import ResponseMeta

        d = dict(src_dict)
        data = DeviceComponentsResponse.from_dict(d.pop("data"))

        meta = ResponseMeta.from_dict(d.pop("meta"))

        get_attachments_response_200 = cls(
            data=data,
            meta=meta,
        )

        get_attachments_response_200.additional_properties = d
        return get_attachments_response_200

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
