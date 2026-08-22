from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar
from uuid import UUID

from attrs import define as _attrs_define

T = TypeVar("T", bound="ReorderLayersRequest")


@_attrs_define
class ReorderLayersRequest:
    """`PATCH /scene/zones/{zone}/layers/order` — reorder the stack.

    `order` names every layer in the zone exactly once, bottom to top;
    anything else is a validation error.

        Attributes:
            order (list[UUID]):
    """

    order: list[UUID]

    def to_dict(self) -> dict[str, Any]:
        order = []
        for order_item_data in self.order:
            order_item = str(order_item_data)
            order.append(order_item)

        field_dict: dict[str, Any] = {}

        field_dict.update(
            {
                "order": order,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        order = []
        _order = d.pop("order")
        for order_item_data in _order:
            order_item = UUID(order_item_data)

            order.append(order_item)

        reorder_layers_request = cls(
            order=order,
        )

        return reorder_layers_request
