from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.device_auth_summary import DeviceAuthSummary
    from ..models.zone_summary import ZoneSummary


T = TypeVar("T", bound="DeviceSummary")


@_attrs_define
class DeviceSummary:
    """
    Attributes:
        backend (str):
        brightness (int):
        id (str):
        layout_device_id (str):
        name (str):
        status (str):
        total_leds (int):
        zones (list[ZoneSummary]):
        auth (DeviceAuthSummary | None | Unset):
        connection_label (None | str | Unset):
        firmware_version (None | str | Unset):
        network_hostname (None | str | Unset):
        network_ip (None | str | Unset):
    """

    backend: str
    brightness: int
    id: str
    layout_device_id: str
    name: str
    status: str
    total_leds: int
    zones: list[ZoneSummary]
    auth: DeviceAuthSummary | None | Unset = UNSET
    connection_label: None | str | Unset = UNSET
    firmware_version: None | str | Unset = UNSET
    network_hostname: None | str | Unset = UNSET
    network_ip: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.device_auth_summary import DeviceAuthSummary

        backend = self.backend

        brightness = self.brightness

        id = self.id

        layout_device_id = self.layout_device_id

        name = self.name

        status = self.status

        total_leds = self.total_leds

        zones = []
        for zones_item_data in self.zones:
            zones_item = zones_item_data.to_dict()
            zones.append(zones_item)

        auth: dict[str, Any] | None | Unset
        if isinstance(self.auth, Unset):
            auth = UNSET
        elif isinstance(self.auth, DeviceAuthSummary):
            auth = self.auth.to_dict()
        else:
            auth = self.auth

        connection_label: None | str | Unset
        if isinstance(self.connection_label, Unset):
            connection_label = UNSET
        else:
            connection_label = self.connection_label

        firmware_version: None | str | Unset
        if isinstance(self.firmware_version, Unset):
            firmware_version = UNSET
        else:
            firmware_version = self.firmware_version

        network_hostname: None | str | Unset
        if isinstance(self.network_hostname, Unset):
            network_hostname = UNSET
        else:
            network_hostname = self.network_hostname

        network_ip: None | str | Unset
        if isinstance(self.network_ip, Unset):
            network_ip = UNSET
        else:
            network_ip = self.network_ip

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "backend": backend,
                "brightness": brightness,
                "id": id,
                "layout_device_id": layout_device_id,
                "name": name,
                "status": status,
                "total_leds": total_leds,
                "zones": zones,
            }
        )
        if auth is not UNSET:
            field_dict["auth"] = auth
        if connection_label is not UNSET:
            field_dict["connection_label"] = connection_label
        if firmware_version is not UNSET:
            field_dict["firmware_version"] = firmware_version
        if network_hostname is not UNSET:
            field_dict["network_hostname"] = network_hostname
        if network_ip is not UNSET:
            field_dict["network_ip"] = network_ip

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.device_auth_summary import DeviceAuthSummary
        from ..models.zone_summary import ZoneSummary

        d = dict(src_dict)
        backend = d.pop("backend")

        brightness = d.pop("brightness")

        id = d.pop("id")

        layout_device_id = d.pop("layout_device_id")

        name = d.pop("name")

        status = d.pop("status")

        total_leds = d.pop("total_leds")

        zones = []
        _zones = d.pop("zones")
        for zones_item_data in _zones:
            zones_item = ZoneSummary.from_dict(zones_item_data)

            zones.append(zones_item)

        def _parse_auth(data: object) -> DeviceAuthSummary | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                auth_type_1 = DeviceAuthSummary.from_dict(data)

                return auth_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(DeviceAuthSummary | None | Unset, data)

        auth = _parse_auth(d.pop("auth", UNSET))

        def _parse_connection_label(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        connection_label = _parse_connection_label(d.pop("connection_label", UNSET))

        def _parse_firmware_version(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        firmware_version = _parse_firmware_version(d.pop("firmware_version", UNSET))

        def _parse_network_hostname(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        network_hostname = _parse_network_hostname(d.pop("network_hostname", UNSET))

        def _parse_network_ip(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        network_ip = _parse_network_ip(d.pop("network_ip", UNSET))

        device_summary = cls(
            backend=backend,
            brightness=brightness,
            id=id,
            layout_device_id=layout_device_id,
            name=name,
            status=status,
            total_leds=total_leds,
            zones=zones,
            auth=auth,
            connection_label=connection_label,
            firmware_version=firmware_version,
            network_hostname=network_hostname,
            network_ip=network_ip,
        )

        device_summary.additional_properties = d
        return device_summary

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
