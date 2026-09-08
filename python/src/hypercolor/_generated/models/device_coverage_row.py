from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.coverage_active import CoverageActive
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.coverage_bridge_device import CoverageBridgeDevice
    from ..models.coverage_identity import CoverageIdentity
    from ..models.coverage_native_device import CoverageNativeDevice


T = TypeVar("T", bound="DeviceCoverageRow")


@_attrs_define
class DeviceCoverageRow:
    """One physical device across the native registry, bridge routes, and the
    unclaimed inventory.

        Attributes:
            active (CoverageActive): Which stack currently owns the hardware in one coverage row.
            identity (CoverageIdentity): The physical-device identity a coverage row was joined on.
            unclaimed (bool): Whether the unclaimed USB inventory also lists this hardware.
            bridge (CoverageBridgeDevice | None | Unset):
            native (CoverageNativeDevice | None | Unset):
    """

    active: CoverageActive
    identity: CoverageIdentity
    unclaimed: bool
    bridge: CoverageBridgeDevice | None | Unset = UNSET
    native: CoverageNativeDevice | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.coverage_bridge_device import CoverageBridgeDevice
        from ..models.coverage_native_device import CoverageNativeDevice

        active = self.active.value

        identity = self.identity.to_dict()

        unclaimed = self.unclaimed

        bridge: dict[str, Any] | None | Unset
        if isinstance(self.bridge, Unset):
            bridge = UNSET
        elif isinstance(self.bridge, CoverageBridgeDevice):
            bridge = self.bridge.to_dict()
        else:
            bridge = self.bridge

        native: dict[str, Any] | None | Unset
        if isinstance(self.native, Unset):
            native = UNSET
        elif isinstance(self.native, CoverageNativeDevice):
            native = self.native.to_dict()
        else:
            native = self.native

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "active": active,
                "identity": identity,
                "unclaimed": unclaimed,
            }
        )
        if bridge is not UNSET:
            field_dict["bridge"] = bridge
        if native is not UNSET:
            field_dict["native"] = native

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.coverage_bridge_device import CoverageBridgeDevice
        from ..models.coverage_identity import CoverageIdentity
        from ..models.coverage_native_device import CoverageNativeDevice

        d = dict(src_dict)
        active = CoverageActive(d.pop("active"))

        identity = CoverageIdentity.from_dict(d.pop("identity"))

        unclaimed = d.pop("unclaimed")

        def _parse_bridge(data: object) -> CoverageBridgeDevice | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                bridge_type_1 = CoverageBridgeDevice.from_dict(data)

                return bridge_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(CoverageBridgeDevice | None | Unset, data)

        bridge = _parse_bridge(d.pop("bridge", UNSET))

        def _parse_native(data: object) -> CoverageNativeDevice | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                native_type_1 = CoverageNativeDevice.from_dict(data)

                return native_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(CoverageNativeDevice | None | Unset, data)

        native = _parse_native(d.pop("native", UNSET))

        device_coverage_row = cls(
            active=active,
            identity=identity,
            unclaimed=unclaimed,
            bridge=bridge,
            native=native,
        )

        device_coverage_row.additional_properties = d
        return device_coverage_row

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
