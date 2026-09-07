from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="ServerInfo")


@_attrs_define
class ServerInfo:
    """
    Attributes:
        auth_required (bool):
        device_count (int):
        instance_id (str):
        instance_name (str):
        version (str):
        server_session_id (None | str | Unset):
    """

    auth_required: bool
    device_count: int
    instance_id: str
    instance_name: str
    version: str
    server_session_id: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        auth_required = self.auth_required

        device_count = self.device_count

        instance_id = self.instance_id

        instance_name = self.instance_name

        version = self.version

        server_session_id: None | str | Unset
        if isinstance(self.server_session_id, Unset):
            server_session_id = UNSET
        else:
            server_session_id = self.server_session_id

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "auth_required": auth_required,
                "device_count": device_count,
                "instance_id": instance_id,
                "instance_name": instance_name,
                "version": version,
            }
        )
        if server_session_id is not UNSET:
            field_dict["server_session_id"] = server_session_id

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        auth_required = d.pop("auth_required")

        device_count = d.pop("device_count")

        instance_id = d.pop("instance_id")

        instance_name = d.pop("instance_name")

        version = d.pop("version")

        def _parse_server_session_id(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        server_session_id = _parse_server_session_id(d.pop("server_session_id", UNSET))

        server_info = cls(
            auth_required=auth_required,
            device_count=device_count,
            instance_id=instance_id,
            instance_name=instance_name,
            version=version,
            server_session_id=server_session_id,
        )

        server_info.additional_properties = d
        return server_info

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
