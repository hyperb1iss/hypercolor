from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define

from ..models.driver_control_value_kind import DriverControlValueKind
from ..types import UNSET, Unset

T = TypeVar("T", bound="DriverControlValue")


@_attrs_define
class DriverControlValue:
    """Typed value payload matching a [`ControlValueType`].

    Attributes:
        kind (DriverControlValueKind):
        value (Any | Unset):
    """

    kind: DriverControlValueKind
    value: Any | Unset = UNSET

    def to_dict(self) -> dict[str, Any]:
        kind = self.kind.value

        value = self.value

        field_dict: dict[str, Any] = {}

        field_dict.update(
            {
                "kind": kind,
            }
        )
        if value is not UNSET:
            field_dict["value"] = value

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        kind = DriverControlValueKind(d.pop("kind"))

        value = d.pop("value", UNSET)

        driver_control_value = cls(
            kind=kind,
            value=value,
        )

        return driver_control_value
