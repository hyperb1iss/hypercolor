from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.discovery_completed_response_status import (
    DiscoveryCompletedResponseStatus,
)

if TYPE_CHECKING:
    from ..models.discovery_scan_result import DiscoveryScanResult


T = TypeVar("T", bound="DiscoveryCompletedResponse")


@_attrs_define
class DiscoveryCompletedResponse:
    """Completed response for a synchronous discovery scan.

    Attributes:
        result (DiscoveryScanResult): Detailed result from a completed discovery scan.
        scan_id (str):
        status (DiscoveryCompletedResponseStatus):
    """

    result: DiscoveryScanResult
    scan_id: str
    status: DiscoveryCompletedResponseStatus
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        result = self.result.to_dict()

        scan_id = self.scan_id

        status = self.status.value

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "result": result,
                "scan_id": scan_id,
                "status": status,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.discovery_scan_result import DiscoveryScanResult

        d = dict(src_dict)
        result = DiscoveryScanResult.from_dict(d.pop("result"))

        scan_id = d.pop("scan_id")

        status = DiscoveryCompletedResponseStatus(d.pop("status"))

        discovery_completed_response = cls(
            result=result,
            scan_id=scan_id,
            status=status,
        )

        discovery_completed_response.additional_properties = d
        return discovery_completed_response

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
