from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define

if TYPE_CHECKING:
    from ..models.output import Output


T = TypeVar("T", bound="MemberState")


@_attrs_define
class MemberState:
    """One output's complete authored state for reversible membership edits.

    Attributes:
        index (int): Position in the owning zone's ordered output list.
        output (Output): A device output: the spatial binding between a physical device and a
            region of the effect canvas.

            The output's bounding rectangle is defined by `position` (center) and
            `size` (width, height), both in normalized `[0.0, 1.0]` canvas coordinates.
            LED positions within the output are computed from the `topology` and stored
            in `led_positions` as output-local normalized coordinates.
        zone_id (str):
    """

    index: int
    output: Output
    zone_id: str

    def to_dict(self) -> dict[str, Any]:
        index = self.index

        output = self.output.to_dict()

        zone_id = self.zone_id

        field_dict: dict[str, Any] = {}

        field_dict.update(
            {
                "index": index,
                "output": output,
                "zone_id": zone_id,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.output import Output

        d = dict(src_dict)
        index = d.pop("index")

        output = Output.from_dict(d.pop("output"))

        zone_id = d.pop("zone_id")

        member_state = cls(
            index=index,
            output=output,
            zone_id=zone_id,
        )

        return member_state
