from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="ConfigMutationResponse")


@_attrs_define
class ConfigMutationResponse:
    """The outcome of a config write, reset, or whole-config reset.

    Attributes:
        live (bool): Whether the daemon re-applied the change to a running subsystem.
        path (str): The config file the write landed in.
        pending_restart (list[str]): Restart-classified roots whose persisted value now differs from
            the one the daemon booted with.
        requires_restart (bool): Whether the registry classifies this key as boot-frozen, so the
            persisted value only takes effect at the next daemon start.
        key (None | str | Unset): The mutated key, or null for a whole-config reset.
        value (Any | Unset): The effective value after the write, rendered like any read.
            Null for a whole-config reset, whose payload spans every key.
    """

    live: bool
    path: str
    pending_restart: list[str]
    requires_restart: bool
    key: None | str | Unset = UNSET
    value: Any | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        live = self.live

        path = self.path

        pending_restart = self.pending_restart

        requires_restart = self.requires_restart

        key: None | str | Unset
        if isinstance(self.key, Unset):
            key = UNSET
        else:
            key = self.key

        value = self.value

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "live": live,
                "path": path,
                "pending_restart": pending_restart,
                "requires_restart": requires_restart,
            }
        )
        if key is not UNSET:
            field_dict["key"] = key
        if value is not UNSET:
            field_dict["value"] = value

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        live = d.pop("live")

        path = d.pop("path")

        pending_restart = cast(list[str], d.pop("pending_restart"))

        requires_restart = d.pop("requires_restart")

        def _parse_key(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        key = _parse_key(d.pop("key", UNSET))

        value = d.pop("value", UNSET)

        config_mutation_response = cls(
            live=live,
            path=path,
            pending_restart=pending_restart,
            requires_restart=requires_restart,
            key=key,
            value=value,
        )

        config_mutation_response.additional_properties = d
        return config_mutation_response

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
