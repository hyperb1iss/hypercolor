from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="BridgeDeviceSummary")


@_attrs_define
class BridgeDeviceSummary:
    """How an out-of-process bridge (OpenRGB) reaches one device.

    Filled from the bridge driver's discovery metadata. `output_enabled`
    is the effective value: a route the daemon's conflict guard has
    output-disabled reports `false` here with the guard's reason, even
    when the bridge itself still advertises the controller as writable.

        Attributes:
            output_enabled (bool): Whether frames may be written through this route.
            controller_index (int | None | Unset): Controller index on that server.
            detector_class (None | str | Unset): The bridge-side detector that produced the controller.
            disabled_reason (None | str | Unset): Why output is disabled, when it is.
            endpoint (None | str | Unset): Bridge server endpoint (`host:port`).
            fingerprint (None | str | Unset): The bridge's stable route fingerprint
                (`bridge:openrgb:<endpoint>:serial:<SERIAL>` or `...:location:<LOCATION>`),
                the key `drivers.openrgb.zone_sizes` entries use.
            identity_confidence (None | str | Unset): How confident the bridge is that this route maps to one physical
                device across restarts (`stable`, `heuristic`, ...).
            protocol_version (int | None | Unset): Negotiated bridge protocol version.
    """

    output_enabled: bool
    controller_index: int | None | Unset = UNSET
    detector_class: None | str | Unset = UNSET
    disabled_reason: None | str | Unset = UNSET
    endpoint: None | str | Unset = UNSET
    fingerprint: None | str | Unset = UNSET
    identity_confidence: None | str | Unset = UNSET
    protocol_version: int | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        output_enabled = self.output_enabled

        controller_index: int | None | Unset
        if isinstance(self.controller_index, Unset):
            controller_index = UNSET
        else:
            controller_index = self.controller_index

        detector_class: None | str | Unset
        if isinstance(self.detector_class, Unset):
            detector_class = UNSET
        else:
            detector_class = self.detector_class

        disabled_reason: None | str | Unset
        if isinstance(self.disabled_reason, Unset):
            disabled_reason = UNSET
        else:
            disabled_reason = self.disabled_reason

        endpoint: None | str | Unset
        if isinstance(self.endpoint, Unset):
            endpoint = UNSET
        else:
            endpoint = self.endpoint

        fingerprint: None | str | Unset
        if isinstance(self.fingerprint, Unset):
            fingerprint = UNSET
        else:
            fingerprint = self.fingerprint

        identity_confidence: None | str | Unset
        if isinstance(self.identity_confidence, Unset):
            identity_confidence = UNSET
        else:
            identity_confidence = self.identity_confidence

        protocol_version: int | None | Unset
        if isinstance(self.protocol_version, Unset):
            protocol_version = UNSET
        else:
            protocol_version = self.protocol_version

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "output_enabled": output_enabled,
            }
        )
        if controller_index is not UNSET:
            field_dict["controller_index"] = controller_index
        if detector_class is not UNSET:
            field_dict["detector_class"] = detector_class
        if disabled_reason is not UNSET:
            field_dict["disabled_reason"] = disabled_reason
        if endpoint is not UNSET:
            field_dict["endpoint"] = endpoint
        if fingerprint is not UNSET:
            field_dict["fingerprint"] = fingerprint
        if identity_confidence is not UNSET:
            field_dict["identity_confidence"] = identity_confidence
        if protocol_version is not UNSET:
            field_dict["protocol_version"] = protocol_version

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        output_enabled = d.pop("output_enabled")

        def _parse_controller_index(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        controller_index = _parse_controller_index(d.pop("controller_index", UNSET))

        def _parse_detector_class(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        detector_class = _parse_detector_class(d.pop("detector_class", UNSET))

        def _parse_disabled_reason(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        disabled_reason = _parse_disabled_reason(d.pop("disabled_reason", UNSET))

        def _parse_endpoint(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        endpoint = _parse_endpoint(d.pop("endpoint", UNSET))

        def _parse_fingerprint(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        fingerprint = _parse_fingerprint(d.pop("fingerprint", UNSET))

        def _parse_identity_confidence(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        identity_confidence = _parse_identity_confidence(
            d.pop("identity_confidence", UNSET)
        )

        def _parse_protocol_version(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        protocol_version = _parse_protocol_version(d.pop("protocol_version", UNSET))

        bridge_device_summary = cls(
            output_enabled=output_enabled,
            controller_index=controller_index,
            detector_class=detector_class,
            disabled_reason=disabled_reason,
            endpoint=endpoint,
            fingerprint=fingerprint,
            identity_confidence=identity_confidence,
            protocol_version=protocol_version,
        )

        bridge_device_summary.additional_properties = d
        return bridge_device_summary

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
