from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.member_state import MemberState


T = TypeVar("T", bound="MemberEdit")


@_attrs_define
class MemberEdit:
    """An atomic add, move, placement change, or removal of one output.

    Preconditions compare membership identity and public placement fields.
    The response replaces both sides with complete authoritative snapshots,
    preserving fields absent from the scene document for later restoration.

        Attributes:
            after (MemberState | None | Unset):
            before (MemberState | None | Unset):
    """

    after: MemberState | None | Unset = UNSET
    before: MemberState | None | Unset = UNSET

    def to_dict(self) -> dict[str, Any]:
        from ..models.member_state import MemberState

        after: dict[str, Any] | None | Unset
        if isinstance(self.after, Unset):
            after = UNSET
        elif isinstance(self.after, MemberState):
            after = self.after.to_dict()
        else:
            after = self.after

        before: dict[str, Any] | None | Unset
        if isinstance(self.before, Unset):
            before = UNSET
        elif isinstance(self.before, MemberState):
            before = self.before.to_dict()
        else:
            before = self.before

        field_dict: dict[str, Any] = {}

        field_dict.update({})
        if after is not UNSET:
            field_dict["after"] = after
        if before is not UNSET:
            field_dict["before"] = before

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.member_state import MemberState

        d = dict(src_dict)

        def _parse_after(data: object) -> MemberState | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                after_type_1 = MemberState.from_dict(data)

                return after_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(MemberState | None | Unset, data)

        after = _parse_after(d.pop("after", UNSET))

        def _parse_before(data: object) -> MemberState | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                before_type_1 = MemberState.from_dict(data)

                return before_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(MemberState | None | Unset, data)

        before = _parse_before(d.pop("before", UNSET))

        member_edit = cls(
            after=after,
            before=before,
        )

        return member_edit
