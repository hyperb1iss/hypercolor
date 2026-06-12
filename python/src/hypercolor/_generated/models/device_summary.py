from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.device_auth_summary import DeviceAuthSummary
    from ..models.device_connection_summary import DeviceConnectionSummary
    from ..models.device_origin import DeviceOrigin
    from ..models.driver_presentation import DriverPresentation
    from ..models.zone_summary import ZoneSummary


T = TypeVar("T", bound="DeviceSummary")


@_attrs_define
class DeviceSummary:
    """One device in the list/detail responses.

    Attributes:
        brightness (int):
        id (str):
        layout_device_id (str):
        name (str):
        origin (DeviceOrigin): Origin metadata that separates device ownership from output routing.
        presentation (DriverPresentation): API and UI presentation metadata for a driver module.
        status (str):
        total_leds (int):
        auth (DeviceAuthSummary | None | Unset):
        connection (DeviceConnectionSummary | Unset): Transport details for one device.
        firmware_version (None | str | Unset):
        zones (list[ZoneSummary] | Unset):
    """

    brightness: int
    id: str
    layout_device_id: str
    name: str
    origin: DeviceOrigin
    presentation: DriverPresentation
    status: str
    total_leds: int
    auth: DeviceAuthSummary | None | Unset = UNSET
    connection: DeviceConnectionSummary | Unset = UNSET
    firmware_version: None | str | Unset = UNSET
    zones: list[ZoneSummary] | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.device_auth_summary import DeviceAuthSummary

        brightness = self.brightness

        id = self.id

        layout_device_id = self.layout_device_id

        name = self.name

        origin = self.origin.to_dict()

        presentation = self.presentation.to_dict()

        status = self.status

        total_leds = self.total_leds

        auth: dict[str, Any] | None | Unset
        if isinstance(self.auth, Unset):
            auth = UNSET
        elif isinstance(self.auth, DeviceAuthSummary):
            auth = self.auth.to_dict()
        else:
            auth = self.auth

        connection: dict[str, Any] | Unset = UNSET
        if not isinstance(self.connection, Unset):
            connection = self.connection.to_dict()

        firmware_version: None | str | Unset
        if isinstance(self.firmware_version, Unset):
            firmware_version = UNSET
        else:
            firmware_version = self.firmware_version

        zones: list[dict[str, Any]] | Unset = UNSET
        if not isinstance(self.zones, Unset):
            zones = []
            for zones_item_data in self.zones:
                zones_item = zones_item_data.to_dict()
                zones.append(zones_item)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "brightness": brightness,
                "id": id,
                "layout_device_id": layout_device_id,
                "name": name,
                "origin": origin,
                "presentation": presentation,
                "status": status,
                "total_leds": total_leds,
            }
        )
        if auth is not UNSET:
            field_dict["auth"] = auth
        if connection is not UNSET:
            field_dict["connection"] = connection
        if firmware_version is not UNSET:
            field_dict["firmware_version"] = firmware_version
        if zones is not UNSET:
            field_dict["zones"] = zones

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.device_auth_summary import DeviceAuthSummary
        from ..models.device_connection_summary import DeviceConnectionSummary
        from ..models.device_origin import DeviceOrigin
        from ..models.driver_presentation import DriverPresentation
        from ..models.zone_summary import ZoneSummary

        d = dict(src_dict)
        brightness = d.pop("brightness")

        id = d.pop("id")

        layout_device_id = d.pop("layout_device_id")

        name = d.pop("name")

        origin = DeviceOrigin.from_dict(d.pop("origin"))

        presentation = DriverPresentation.from_dict(d.pop("presentation"))

        status = d.pop("status")

        total_leds = d.pop("total_leds")

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

        _connection = d.pop("connection", UNSET)
        connection: DeviceConnectionSummary | Unset
        if isinstance(_connection, Unset):
            connection = UNSET
        else:
            connection = DeviceConnectionSummary.from_dict(_connection)

        def _parse_firmware_version(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        firmware_version = _parse_firmware_version(d.pop("firmware_version", UNSET))

        _zones = d.pop("zones", UNSET)
        zones: list[ZoneSummary] | Unset = UNSET
        if _zones is not UNSET:
            zones = []
            for zones_item_data in _zones:
                zones_item = ZoneSummary.from_dict(zones_item_data)

                zones.append(zones_item)

        device_summary = cls(
            brightness=brightness,
            id=id,
            layout_device_id=layout_device_id,
            name=name,
            origin=origin,
            presentation=presentation,
            status=status,
            total_leds=total_leds,
            auth=auth,
            connection=connection,
            firmware_version=firmware_version,
            zones=zones,
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
