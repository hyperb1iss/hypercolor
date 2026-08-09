from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

T = TypeVar("T", bound="UnresolvedBindingSummary")


@_attrs_define
class UnresolvedBindingSummary:
    """One layout binding with no attached device behind it.

    Attributes:
        layout_device_id (str): The layout binding id the zones reference.
        layout_ids (list[str]): The layouts whose zones reference it.
        rebindable (bool): Whether a recorded identity exists for this binding, which is what
            a durable re-bind needs to inherit.
    """

    layout_device_id: str
    layout_ids: list[str]
    rebindable: bool
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        layout_device_id = self.layout_device_id

        layout_ids = self.layout_ids

        rebindable = self.rebindable

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "layout_device_id": layout_device_id,
                "layout_ids": layout_ids,
                "rebindable": rebindable,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        layout_device_id = d.pop("layout_device_id")

        layout_ids = cast(list[str], d.pop("layout_ids"))

        rebindable = d.pop("rebindable")

        unresolved_binding_summary = cls(
            layout_device_id=layout_device_id,
            layout_ids=layout_ids,
            rebindable=rebindable,
        )

        unresolved_binding_summary.additional_properties = d
        return unresolved_binding_summary

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
