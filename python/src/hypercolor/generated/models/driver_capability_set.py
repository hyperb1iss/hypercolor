from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="DriverCapabilitySet")


@_attrs_define
class DriverCapabilitySet:
    """Capability flags exposed by a driver module.

    Attributes:
        backend_factory (bool): Builds an output backend.
        config (bool): Exposes driver-scoped configuration.
        credentials (bool): Stores credentials or authorization material.
        discovery (bool): Discovers devices.
        pairing (bool): Supports pairing or authorization flows.
        presentation (bool): Provides presentation metadata.
        protocol_catalog (bool): Contributes protocols to a shared backend.
        runtime_cache (bool): Keeps runtime cache state.
        controls (bool | Unset): Exposes typed dynamic control surfaces.
    """

    backend_factory: bool
    config: bool
    credentials: bool
    discovery: bool
    pairing: bool
    presentation: bool
    protocol_catalog: bool
    runtime_cache: bool
    controls: bool | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        backend_factory = self.backend_factory

        config = self.config

        credentials = self.credentials

        discovery = self.discovery

        pairing = self.pairing

        presentation = self.presentation

        protocol_catalog = self.protocol_catalog

        runtime_cache = self.runtime_cache

        controls = self.controls

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "backend_factory": backend_factory,
                "config": config,
                "credentials": credentials,
                "discovery": discovery,
                "pairing": pairing,
                "presentation": presentation,
                "protocol_catalog": protocol_catalog,
                "runtime_cache": runtime_cache,
            }
        )
        if controls is not UNSET:
            field_dict["controls"] = controls

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        backend_factory = d.pop("backend_factory")

        config = d.pop("config")

        credentials = d.pop("credentials")

        discovery = d.pop("discovery")

        pairing = d.pop("pairing")

        presentation = d.pop("presentation")

        protocol_catalog = d.pop("protocol_catalog")

        runtime_cache = d.pop("runtime_cache")

        controls = d.pop("controls", UNSET)

        driver_capability_set = cls(
            backend_factory=backend_factory,
            config=config,
            credentials=credentials,
            discovery=discovery,
            pairing=pairing,
            presentation=presentation,
            protocol_catalog=protocol_catalog,
            runtime_cache=runtime_cache,
            controls=controls,
        )

        driver_capability_set.additional_properties = d
        return driver_capability_set

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
