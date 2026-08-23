from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.discovery_completed_response import DiscoveryCompletedResponse
    from ..models.discovery_scanning_response import DiscoveryScanningResponse
    from ..models.response_meta import ResponseMeta


T = TypeVar("T", bound="DiscoverDevicesResponse200")


@_attrs_define
class DiscoverDevicesResponse200:
    """
    Attributes:
        data (DiscoveryCompletedResponse | DiscoveryScanningResponse): Response from `POST /api/v1/devices/discover`.
        meta (ResponseMeta): Response metadata included in every envelope.
    """

    data: DiscoveryCompletedResponse | DiscoveryScanningResponse
    meta: ResponseMeta
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.discovery_scanning_response import DiscoveryScanningResponse

        data: dict[str, Any]
        if isinstance(self.data, DiscoveryScanningResponse):
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
        from ..models.discovery_scanning_response import DiscoveryScanningResponse
        from ..models.response_meta import ResponseMeta

        d = dict(src_dict)

        def _parse_data(
            data: object,
        ) -> DiscoveryCompletedResponse | DiscoveryScanningResponse:
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_discover_response_discovery_scanning_response = (
                    DiscoveryScanningResponse.from_dict(data)
                )

                return componentsschemas_discover_response_discovery_scanning_response
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            if not isinstance(data, dict):
                raise TypeError()
            componentsschemas_discover_response_discovery_completed_response = (
                DiscoveryCompletedResponse.from_dict(data)
            )

            return componentsschemas_discover_response_discovery_completed_response

        data = _parse_data(d.pop("data"))

        meta = ResponseMeta.from_dict(d.pop("meta"))

        discover_devices_response_200 = cls(
            data=data,
            meta=meta,
        )

        discover_devices_response_200.additional_properties = d
        return discover_devices_response_200

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
