from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

if TYPE_CHECKING:
    from ..models.effect_console_entry import EffectConsoleEntry


T = TypeVar("T", bound="EffectConsoleResponse")


@_attrs_define
class EffectConsoleResponse:
    """Response for `GET /api/v1/effects/console`.

    A cursor-based window over the effect console journal rather than a
    paged list: pass `next_seq` back as `after` to read only newer output.

        Attributes:
            entries (list[EffectConsoleEntry]): Matching messages after the cursor, oldest first.
            next_seq (int): Cursor for the next read.
            truncated (bool): Whether output after the requested cursor was evicted before this
                read, so some messages were missed.
    """

    entries: list[EffectConsoleEntry]
    next_seq: int
    truncated: bool
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        entries = []
        for entries_item_data in self.entries:
            entries_item = entries_item_data.to_dict()
            entries.append(entries_item)

        next_seq = self.next_seq

        truncated = self.truncated

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "entries": entries,
                "next_seq": next_seq,
                "truncated": truncated,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.effect_console_entry import EffectConsoleEntry

        d = dict(src_dict)
        entries = []
        _entries = d.pop("entries")
        for entries_item_data in _entries:
            entries_item = EffectConsoleEntry.from_dict(entries_item_data)

            entries.append(entries_item)

        next_seq = d.pop("next_seq")

        truncated = d.pop("truncated")

        effect_console_response = cls(
            entries=entries,
            next_seq=next_seq,
            truncated=truncated,
        )

        effect_console_response.additional_properties = d
        return effect_console_response

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
