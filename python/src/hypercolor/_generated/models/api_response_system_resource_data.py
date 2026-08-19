from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.server_info import ServerInfo
    from ..models.system_status import SystemStatus


T = TypeVar("T", bound="ApiResponseSystemResourceData")


@_attrs_define
class ApiResponseSystemResourceData:
    """
    Attributes:
        identity (ServerInfo):
        status (None | SystemStatus | Unset):
    """

    identity: ServerInfo
    status: None | SystemStatus | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.system_status import SystemStatus

        identity = self.identity.to_dict()

        status: dict[str, Any] | None | Unset
        if isinstance(self.status, Unset):
            status = UNSET
        elif isinstance(self.status, SystemStatus):
            status = self.status.to_dict()
        else:
            status = self.status

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "identity": identity,
            }
        )
        if status is not UNSET:
            field_dict["status"] = status

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.server_info import ServerInfo
        from ..models.system_status import SystemStatus

        d = dict(src_dict)
        identity = ServerInfo.from_dict(d.pop("identity"))

        def _parse_status(data: object) -> None | SystemStatus | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                status_type_1 = SystemStatus.from_dict(data)

                return status_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | SystemStatus | Unset, data)

        status = _parse_status(d.pop("status", UNSET))

        api_response_system_resource_data = cls(
            identity=identity,
            status=status,
        )

        api_response_system_resource_data.additional_properties = d
        return api_response_system_resource_data

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
