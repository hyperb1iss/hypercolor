from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar
from uuid import UUID

from attrs import define as _attrs_define
from typing_extensions import Self

if TYPE_CHECKING:
    from ..models.layer_scope import LayerScope
    from ..models.layer_slot import LayerSlot


T = TypeVar("T", bound="LayerMove")


@_attrs_define
class LayerMove:
    """One scoped layer a membership edit carried with its outputs, or
    narrowed because some of them left.

    `from.index` is the layer's position before the edit and `to.index`
    its position after it. A layer whose outputs all moved changes zone
    and keeps its scope; a layer that lost only some of them keeps its
    zone and loses those ids. [`Self::reversed`] is the record that undoes
    it.

        Attributes:
            from_ (LayerSlot): A layer's place in a scene: the zone that owns it and its position in
                that zone's bottom-to-top stack.
            layer_id (UUID): Stable identifier for a layer within a zone.
            scope_after (LayerScope): The outputs one layer is limited to.
            scope_before (LayerScope): The outputs one layer is limited to.
            to (LayerSlot): A layer's place in a scene: the zone that owns it and its position in
                that zone's bottom-to-top stack.
    """

    from_: LayerSlot
    layer_id: UUID
    scope_after: LayerScope
    scope_before: LayerScope
    to: LayerSlot

    def to_dict(self) -> dict[str, Any]:
        from_ = self.from_.to_dict()

        layer_id = str(self.layer_id)

        scope_after = self.scope_after.to_dict()

        scope_before = self.scope_before.to_dict()

        to = self.to.to_dict()

        field_dict: dict[str, Any] = {}

        field_dict.update(
            {
                "from": from_,
                "layer_id": layer_id,
                "scope_after": scope_after,
                "scope_before": scope_before,
                "to": to,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.layer_scope import LayerScope
        from ..models.layer_slot import LayerSlot

        d = dict(src_dict)
        from_ = LayerSlot.from_dict(d.pop("from"))

        layer_id = UUID(d.pop("layer_id"))

        scope_after = LayerScope.from_dict(d.pop("scope_after"))

        scope_before = LayerScope.from_dict(d.pop("scope_before"))

        to = LayerSlot.from_dict(d.pop("to"))

        layer_move = cls(
            from_=from_,
            layer_id=layer_id,
            scope_after=scope_after,
            scope_before=scope_before,
            to=to,
        )

        return layer_move
