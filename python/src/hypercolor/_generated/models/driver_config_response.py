from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.driver_config_entry import DriverConfigEntry


T = TypeVar("T", bound="DriverConfigResponse")


@_attrs_define
class DriverConfigResponse:
    """
    Attributes:
        config_key (str):
        configurable (bool):
        current (DriverConfigEntry): Host-owned wrapper around one driver's settings.
        driver_id (str):
        default (DriverConfigEntry | None | Unset):
    """

    config_key: str
    configurable: bool
    current: DriverConfigEntry
    driver_id: str
    default: DriverConfigEntry | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.driver_config_entry import DriverConfigEntry

        config_key = self.config_key

        configurable = self.configurable

        current = self.current.to_dict()

        driver_id = self.driver_id

        default: dict[str, Any] | None | Unset
        if isinstance(self.default, Unset):
            default = UNSET
        elif isinstance(self.default, DriverConfigEntry):
            default = self.default.to_dict()
        else:
            default = self.default

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "config_key": config_key,
                "configurable": configurable,
                "current": current,
                "driver_id": driver_id,
            }
        )
        if default is not UNSET:
            field_dict["default"] = default

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.driver_config_entry import DriverConfigEntry

        d = dict(src_dict)
        config_key = d.pop("config_key")

        configurable = d.pop("configurable")

        current = DriverConfigEntry.from_dict(d.pop("current"))

        driver_id = d.pop("driver_id")

        def _parse_default(data: object) -> DriverConfigEntry | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                default_type_1 = DriverConfigEntry.from_dict(data)

                return default_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(DriverConfigEntry | None | Unset, data)

        default = _parse_default(d.pop("default", UNSET))

        driver_config_response = cls(
            config_key=config_key,
            configurable=configurable,
            current=current,
            driver_id=driver_id,
            default=default,
        )

        driver_config_response.additional_properties = d
        return driver_config_response

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
