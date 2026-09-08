from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="OpenRgbEndpointStatus")


@_attrs_define
class OpenRgbEndpointStatus:
    """SDK handshake result for one configured endpoint.

    Attributes:
        endpoint (str):
        reachable (bool):
        controller_count (int | None | Unset):
        error (None | str | Unset):
        protocol_version (int | None | Unset):
    """

    endpoint: str
    reachable: bool
    controller_count: int | None | Unset = UNSET
    error: None | str | Unset = UNSET
    protocol_version: int | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        endpoint = self.endpoint

        reachable = self.reachable

        controller_count: int | None | Unset
        if isinstance(self.controller_count, Unset):
            controller_count = UNSET
        else:
            controller_count = self.controller_count

        error: None | str | Unset
        if isinstance(self.error, Unset):
            error = UNSET
        else:
            error = self.error

        protocol_version: int | None | Unset
        if isinstance(self.protocol_version, Unset):
            protocol_version = UNSET
        else:
            protocol_version = self.protocol_version

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "endpoint": endpoint,
                "reachable": reachable,
            }
        )
        if controller_count is not UNSET:
            field_dict["controller_count"] = controller_count
        if error is not UNSET:
            field_dict["error"] = error
        if protocol_version is not UNSET:
            field_dict["protocol_version"] = protocol_version

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        endpoint = d.pop("endpoint")

        reachable = d.pop("reachable")

        def _parse_controller_count(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        controller_count = _parse_controller_count(d.pop("controller_count", UNSET))

        def _parse_error(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        error = _parse_error(d.pop("error", UNSET))

        def _parse_protocol_version(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        protocol_version = _parse_protocol_version(d.pop("protocol_version", UNSET))

        open_rgb_endpoint_status = cls(
            endpoint=endpoint,
            reachable=reachable,
            controller_count=controller_count,
            error=error,
            protocol_version=protocol_version,
        )

        open_rgb_endpoint_status.additional_properties = d
        return open_rgb_endpoint_status

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
