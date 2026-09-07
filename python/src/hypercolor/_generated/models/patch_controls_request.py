from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.patch_controls_request_values import PatchControlsRequestValues


T = TypeVar("T", bound="PatchControlsRequest")


@_attrs_define
class PatchControlsRequest:
    """The one control-patch shape, used verbatim at every scope: layer
    controls, display face controls, control-surface values
    (Spec 78 §5.7).

    `clear_bindings` is meaningful only where bindings exist (layers);
    other scopes reject a non-empty list with a validation error. A
    patch naming a control key with an active input binding is rejected
    409 `control_bound` unless the same request clears that binding —
    removal and the accompanying values land in one atomic commit
    (Spec 78 §1.6).

        Attributes:
            clear_bindings (list[str] | Unset):
            values (PatchControlsRequestValues | Unset):
    """

    clear_bindings: list[str] | Unset = UNSET
    values: PatchControlsRequestValues | Unset = UNSET

    def to_dict(self) -> dict[str, Any]:
        clear_bindings: list[str] | Unset = UNSET
        if not isinstance(self.clear_bindings, Unset):
            clear_bindings = self.clear_bindings

        values: dict[str, Any] | Unset = UNSET
        if not isinstance(self.values, Unset):
            values = self.values.to_dict()

        field_dict: dict[str, Any] = {}

        field_dict.update({})
        if clear_bindings is not UNSET:
            field_dict["clear_bindings"] = clear_bindings
        if values is not UNSET:
            field_dict["values"] = values

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.patch_controls_request_values import PatchControlsRequestValues

        d = dict(src_dict)
        clear_bindings = cast(list[str], d.pop("clear_bindings", UNSET))

        _values = d.pop("values", UNSET)
        values: PatchControlsRequestValues | Unset
        if isinstance(_values, Unset):
            values = UNSET
        else:
            values = PatchControlsRequestValues.from_dict(_values)

        patch_controls_request = cls(
            clear_bindings=clear_bindings,
            values=values,
        )

        return patch_controls_request
