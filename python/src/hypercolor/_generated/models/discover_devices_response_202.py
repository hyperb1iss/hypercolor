from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.discovery_completed_response import DiscoveryCompletedResponse
    from ..models.discovery_started_response import DiscoveryStartedResponse
    from ..models.response_meta import ResponseMeta


T = TypeVar("T", bound="DiscoverDevicesResponse202")


@_attrs_define
class DiscoverDevicesResponse202:
    """
    Attributes:
        data (DiscoveryCompletedResponse | DiscoveryStartedResponse): Response from `POST /api/v1/devices/discover`.
        meta (ResponseMeta): Response metadata included in every envelope.
    """

    data: DiscoveryCompletedResponse | DiscoveryStartedResponse
    meta: ResponseMeta
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.discovery_started_response import DiscoveryStartedResponse

        data: dict[str, Any]
        if isinstance(self.data, DiscoveryStartedResponse):
            data = self.data.to_dict()
        else:
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
        from ..models.discovery_completed_response import DiscoveryCompletedResponse
        from ..models.discovery_started_response import DiscoveryStartedResponse
        from ..models.response_meta import ResponseMeta

        d = dict(src_dict)

        def _parse_data(
            data: object,
        ) -> DiscoveryCompletedResponse | DiscoveryStartedResponse:
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_discover_response_type_0 = (
                    DiscoveryStartedResponse.from_dict(data)
                )

                return componentsschemas_discover_response_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            if not isinstance(data, dict):
                raise TypeError()
            componentsschemas_discover_response_type_1 = (
                DiscoveryCompletedResponse.from_dict(data)
            )

            return componentsschemas_discover_response_type_1

        data = _parse_data(d.pop("data"))

        meta = ResponseMeta.from_dict(d.pop("meta"))

        discover_devices_response_202 = cls(
            data=data,
            meta=meta,
        )

        discover_devices_response_202.additional_properties = d
        return discover_devices_response_202

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
