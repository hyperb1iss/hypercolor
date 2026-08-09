from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.rebind_candidate_summary import RebindCandidateSummary
    from ..models.unresolved_binding_summary import UnresolvedBindingSummary


T = TypeVar("T", bound="ApiResponseDeviceBindingsResponseData")


@_attrs_define
class ApiResponseDeviceBindingsResponseData:
    """Response for `GET /api/v1/devices/bindings`.

    Attributes:
        candidates (list[RebindCandidateSummary]): Attached devices no layout binding references, offered for re-bind.
        unresolved (list[UnresolvedBindingSummary]): Layout bindings that no attached device currently resolves.
    """

    candidates: list[RebindCandidateSummary]
    unresolved: list[UnresolvedBindingSummary]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        candidates = []
        for candidates_item_data in self.candidates:
            candidates_item = candidates_item_data.to_dict()
            candidates.append(candidates_item)

        unresolved = []
        for unresolved_item_data in self.unresolved:
            unresolved_item = unresolved_item_data.to_dict()
            unresolved.append(unresolved_item)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "candidates": candidates,
                "unresolved": unresolved,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.rebind_candidate_summary import RebindCandidateSummary
        from ..models.unresolved_binding_summary import UnresolvedBindingSummary

        d = dict(src_dict)
        candidates = []
        _candidates = d.pop("candidates")
        for candidates_item_data in _candidates:
            candidates_item = RebindCandidateSummary.from_dict(candidates_item_data)

            candidates.append(candidates_item)

        unresolved = []
        _unresolved = d.pop("unresolved")
        for unresolved_item_data in _unresolved:
            unresolved_item = UnresolvedBindingSummary.from_dict(unresolved_item_data)

            unresolved.append(unresolved_item)

        api_response_device_bindings_response_data = cls(
            candidates=candidates,
            unresolved=unresolved,
        )

        api_response_device_bindings_response_data.additional_properties = d
        return api_response_device_bindings_response_data

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
