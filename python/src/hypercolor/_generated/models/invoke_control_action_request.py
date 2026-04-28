from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.b_tree_map import BTreeMap


T = TypeVar("T", bound="InvokeControlActionRequest")


@_attrs_define
class InvokeControlActionRequest:
    """
    Attributes:
        input_ (BTreeMap | Unset):
    """

    input_: BTreeMap | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        input_: dict[str, Any] | Unset = UNSET
        if not isinstance(self.input_, Unset):
            input_ = self.input_.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if input_ is not UNSET:
            field_dict["input"] = input_

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.b_tree_map import BTreeMap

        d = dict(src_dict)
        _input_ = d.pop("input", UNSET)
        input_: BTreeMap | Unset
        if isinstance(_input_, Unset):
            input_ = UNSET
        else:
            input_ = BTreeMap.from_dict(_input_)

        invoke_control_action_request = cls(
            input_=input_,
        )

        invoke_control_action_request.additional_properties = d
        return invoke_control_action_request

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
