from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.layout_link_summary import LayoutLinkSummary


T = TypeVar("T", bound="EffectLayoutApplyResult")


@_attrs_define
class EffectLayoutApplyResult:
    """
    Attributes:
        applied (bool):
        associated_layout_id (str):
        resolved (bool):
        layout (LayoutLinkSummary | None | Unset):
    """

    applied: bool
    associated_layout_id: str
    resolved: bool
    layout: LayoutLinkSummary | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.layout_link_summary import LayoutLinkSummary

        applied = self.applied

        associated_layout_id = self.associated_layout_id

        resolved = self.resolved

        layout: dict[str, Any] | None | Unset
        if isinstance(self.layout, Unset):
            layout = UNSET
        elif isinstance(self.layout, LayoutLinkSummary):
            layout = self.layout.to_dict()
        else:
            layout = self.layout

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "applied": applied,
                "associated_layout_id": associated_layout_id,
                "resolved": resolved,
            }
        )
        if layout is not UNSET:
            field_dict["layout"] = layout

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.layout_link_summary import LayoutLinkSummary

        d = dict(src_dict)
        applied = d.pop("applied")

        associated_layout_id = d.pop("associated_layout_id")

        resolved = d.pop("resolved")

        def _parse_layout(data: object) -> LayoutLinkSummary | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                layout_type_1 = LayoutLinkSummary.from_dict(data)

                return layout_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(LayoutLinkSummary | None | Unset, data)

        layout = _parse_layout(d.pop("layout", UNSET))

        effect_layout_apply_result = cls(
            applied=applied,
            associated_layout_id=associated_layout_id,
            resolved=resolved,
            layout=layout,
        )

        effect_layout_apply_result.additional_properties = d
        return effect_layout_apply_result

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
