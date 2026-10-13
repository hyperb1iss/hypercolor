from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..types import UNSET, Unset

T = TypeVar("T", bound="EffectConsoleEntry")


@_attrs_define
class EffectConsoleEntry:
    """One message an effect page printed, as `GET /api/v1/effects/console`
    reports it.

        Attributes:
            effect_name (str): Display name of the effect that printed it.
            level (str): Raw console level: `log`, `info`, `warn`, `error`, `debug`, `trace`,
                or `dir`.
            message (str): Message text. File locations in the effect's own page point at the
                author's source file and line, not the engine's runtime copy.
            seq (int): Journal sequence number; strictly increasing within one daemon run.
            source_file (str): File name of the effect's HTML source.
            timestamp_ms (int): Wall-clock time the message was recorded, in Unix milliseconds.
            effect_id (None | str | Unset): Catalog id of that effect, when it is still installed.
            line (int | None | Unset): Line in that file the message points at, when it names one.
    """

    effect_name: str
    level: str
    message: str
    seq: int
    source_file: str
    timestamp_ms: int
    effect_id: None | str | Unset = UNSET
    line: int | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        effect_name = self.effect_name

        level = self.level

        message = self.message

        seq = self.seq

        source_file = self.source_file

        timestamp_ms = self.timestamp_ms

        effect_id: None | str | Unset
        if isinstance(self.effect_id, Unset):
            effect_id = UNSET
        else:
            effect_id = self.effect_id

        line: int | None | Unset
        if isinstance(self.line, Unset):
            line = UNSET
        else:
            line = self.line

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "effect_name": effect_name,
                "level": level,
                "message": message,
                "seq": seq,
                "source_file": source_file,
                "timestamp_ms": timestamp_ms,
            }
        )
        if effect_id is not UNSET:
            field_dict["effect_id"] = effect_id
        if line is not UNSET:
            field_dict["line"] = line

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        effect_name = d.pop("effect_name")

        level = d.pop("level")

        message = d.pop("message")

        seq = d.pop("seq")

        source_file = d.pop("source_file")

        timestamp_ms = d.pop("timestamp_ms")

        def _parse_effect_id(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        effect_id = _parse_effect_id(d.pop("effect_id", UNSET))

        def _parse_line(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        line = _parse_line(d.pop("line", UNSET))

        effect_console_entry = cls(
            effect_name=effect_name,
            level=level,
            message=message,
            seq=seq,
            source_file=source_file,
            timestamp_ms=timestamp_ms,
            effect_id=effect_id,
            line=line,
        )

        effect_console_entry.additional_properties = d
        return effect_console_entry

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
