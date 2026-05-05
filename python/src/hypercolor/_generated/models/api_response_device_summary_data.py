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


T = TypeVar("T", bound="ApiResponseDeviceSummaryData")


@_attrs_define
class ApiResponseDeviceSummaryData:
    """
    Attributes:
        brightness (int):
        connection (DeviceConnectionSummary):
        id (str):
        layout_device_id (str):
        name (str):
        origin (DeviceOrigin): Origin metadata that separates device ownership from output routing.
        presentation (DriverPresentation): API and UI presentation metadata for a driver module.
        status (str):
        total_leds (int):
        zones (list[ZoneSummary]):
        auth (DeviceAuthSummary | None | Unset):
        firmware_version (None | str | Unset):
    """

    brightness: int
    connection: DeviceConnectionSummary
    id: str
    layout_device_id: str
    name: str
    origin: DeviceOrigin
    presentation: DriverPresentation
    status: str
    total_leds: int
    zones: list[ZoneSummary]
    auth: DeviceAuthSummary | None | Unset = UNSET
    firmware_version: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.device_auth_summary import DeviceAuthSummary

        brightness = self.brightness

        connection = self.connection.to_dict()

        id = self.id

        layout_device_id = self.layout_device_id

        name = self.name

        origin = self.origin.to_dict()

        presentation = self.presentation.to_dict()

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

        firmware_version: None | str | Unset
        if isinstance(self.firmware_version, Unset):
            firmware_version = UNSET
        else:
            firmware_version = self.firmware_version

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "brightness": brightness,
                "connection": connection,
                "id": id,
                "layout_device_id": layout_device_id,
                "name": name,
                "origin": origin,
                "presentation": presentation,
                "status": status,
                "total_leds": total_leds,
                "zones": zones,
            }
        )
        if auth is not UNSET:
            field_dict["auth"] = auth
        if firmware_version is not UNSET:
            field_dict["firmware_version"] = firmware_version

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

        connection = DeviceConnectionSummary.from_dict(d.pop("connection"))

        id = d.pop("id")

        layout_device_id = d.pop("layout_device_id")

        name = d.pop("name")

        origin = DeviceOrigin.from_dict(d.pop("origin"))

        presentation = DriverPresentation.from_dict(d.pop("presentation"))

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

        def _parse_firmware_version(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        firmware_version = _parse_firmware_version(d.pop("firmware_version", UNSET))

        api_response_device_summary_data = cls(
            brightness=brightness,
            connection=connection,
            id=id,
            layout_device_id=layout_device_id,
            name=name,
            origin=origin,
            presentation=presentation,
            status=status,
            total_leds=total_leds,
            zones=zones,
            auth=auth,
            firmware_version=firmware_version,
        )

        api_response_device_summary_data.additional_properties = d
        return api_response_device_summary_data

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
