from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define

from ..models.control_value_kind import ControlValueKind
from ..types import UNSET, Unset

T = TypeVar("T", bound="ControlValue")


@_attrs_define
class ControlValue:
    """ControlValue payload

    Attributes:
        kind (ControlValueKind):
        value (Any | Unset):
    """

    kind: ControlValueKind
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
        kind = ControlValueKind(d.pop("kind"))

        value = d.pop("value", UNSET)

        control_value = cls(
            kind=kind,
            value=value,
        )

        return control_value
