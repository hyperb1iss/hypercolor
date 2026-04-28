from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="DiscoverRequest")


@_attrs_define
class DiscoverRequest:
    """
    Attributes:
        backends (list[str] | None | Unset):
        timeout_ms (int | None | Unset):
        wait (bool | None | Unset):
    """

    backends: list[str] | None | Unset = UNSET
    timeout_ms: int | None | Unset = UNSET
    wait: bool | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        backends: list[str] | None | Unset
        if isinstance(self.backends, Unset):
            backends = UNSET
        elif isinstance(self.backends, list):
            backends = self.backends

        else:
            backends = self.backends

        timeout_ms: int | None | Unset
        if isinstance(self.timeout_ms, Unset):
            timeout_ms = UNSET
        else:
            timeout_ms = self.timeout_ms

        wait: bool | None | Unset
        if isinstance(self.wait, Unset):
            wait = UNSET
        else:
            wait = self.wait

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if backends is not UNSET:
            field_dict["backends"] = backends
        if timeout_ms is not UNSET:
            field_dict["timeout_ms"] = timeout_ms
        if wait is not UNSET:
            field_dict["wait"] = wait

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)

        def _parse_backends(data: object) -> list[str] | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, list):
                    raise TypeError()
                backends_type_0 = cast(list[str], data)

                return backends_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(list[str] | None | Unset, data)

        backends = _parse_backends(d.pop("backends", UNSET))

        def _parse_timeout_ms(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        timeout_ms = _parse_timeout_ms(d.pop("timeout_ms", UNSET))

        def _parse_wait(data: object) -> bool | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(bool | None | Unset, data)

        wait = _parse_wait(d.pop("wait", UNSET))

        discover_request = cls(
            backends=backends,
            timeout_ms=timeout_ms,
            wait=wait,
        )

        discover_request.additional_properties = d
        return discover_request

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
