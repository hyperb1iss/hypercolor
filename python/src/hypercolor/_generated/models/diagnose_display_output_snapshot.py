from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="DiagnoseDisplayOutputSnapshot")


@_attrs_define
class DiagnoseDisplayOutputSnapshot:
    """
    Attributes:
        captured_devices (int):
        encode_attempts_total (int):
        encode_avg_ms (float):
        encode_failures_total (int):
        encode_max_ms (float):
        encode_successes_total (int):
        encoded_bytes_total (int):
        encoded_last_bytes (int):
        preview_subscribers (int):
        retry_attempts_total (int):
        write_attempts_total (int):
        write_failures_total (int):
        write_successes_total (int):
        encode_last_ms (float | None | Unset):
        last_failure_age_ms (int | None | Unset):
    """

    captured_devices: int
    encode_attempts_total: int
    encode_avg_ms: float
    encode_failures_total: int
    encode_max_ms: float
    encode_successes_total: int
    encoded_bytes_total: int
    encoded_last_bytes: int
    preview_subscribers: int
    retry_attempts_total: int
    write_attempts_total: int
    write_failures_total: int
    write_successes_total: int
    encode_last_ms: float | None | Unset = UNSET
    last_failure_age_ms: int | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        captured_devices = self.captured_devices

        encode_attempts_total = self.encode_attempts_total

        encode_avg_ms = self.encode_avg_ms

        encode_failures_total = self.encode_failures_total

        encode_max_ms = self.encode_max_ms

        encode_successes_total = self.encode_successes_total

        encoded_bytes_total = self.encoded_bytes_total

        encoded_last_bytes = self.encoded_last_bytes

        preview_subscribers = self.preview_subscribers

        retry_attempts_total = self.retry_attempts_total

        write_attempts_total = self.write_attempts_total

        write_failures_total = self.write_failures_total

        write_successes_total = self.write_successes_total

        encode_last_ms: float | None | Unset
        if isinstance(self.encode_last_ms, Unset):
            encode_last_ms = UNSET
        else:
            encode_last_ms = self.encode_last_ms

        last_failure_age_ms: int | None | Unset
        if isinstance(self.last_failure_age_ms, Unset):
            last_failure_age_ms = UNSET
        else:
            last_failure_age_ms = self.last_failure_age_ms

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "captured_devices": captured_devices,
                "encode_attempts_total": encode_attempts_total,
                "encode_avg_ms": encode_avg_ms,
                "encode_failures_total": encode_failures_total,
                "encode_max_ms": encode_max_ms,
                "encode_successes_total": encode_successes_total,
                "encoded_bytes_total": encoded_bytes_total,
                "encoded_last_bytes": encoded_last_bytes,
                "preview_subscribers": preview_subscribers,
                "retry_attempts_total": retry_attempts_total,
                "write_attempts_total": write_attempts_total,
                "write_failures_total": write_failures_total,
                "write_successes_total": write_successes_total,
            }
        )
        if encode_last_ms is not UNSET:
            field_dict["encode_last_ms"] = encode_last_ms
        if last_failure_age_ms is not UNSET:
            field_dict["last_failure_age_ms"] = last_failure_age_ms

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        captured_devices = d.pop("captured_devices")

        encode_attempts_total = d.pop("encode_attempts_total")

        encode_avg_ms = d.pop("encode_avg_ms")

        encode_failures_total = d.pop("encode_failures_total")

        encode_max_ms = d.pop("encode_max_ms")

        encode_successes_total = d.pop("encode_successes_total")

        encoded_bytes_total = d.pop("encoded_bytes_total")

        encoded_last_bytes = d.pop("encoded_last_bytes")

        preview_subscribers = d.pop("preview_subscribers")

        retry_attempts_total = d.pop("retry_attempts_total")

        write_attempts_total = d.pop("write_attempts_total")

        write_failures_total = d.pop("write_failures_total")

        write_successes_total = d.pop("write_successes_total")

        def _parse_encode_last_ms(data: object) -> float | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(float | None | Unset, data)

        encode_last_ms = _parse_encode_last_ms(d.pop("encode_last_ms", UNSET))

        def _parse_last_failure_age_ms(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        last_failure_age_ms = _parse_last_failure_age_ms(
            d.pop("last_failure_age_ms", UNSET)
        )

        diagnose_display_output_snapshot = cls(
            captured_devices=captured_devices,
            encode_attempts_total=encode_attempts_total,
            encode_avg_ms=encode_avg_ms,
            encode_failures_total=encode_failures_total,
            encode_max_ms=encode_max_ms,
            encode_successes_total=encode_successes_total,
            encoded_bytes_total=encoded_bytes_total,
            encoded_last_bytes=encoded_last_bytes,
            preview_subscribers=preview_subscribers,
            retry_attempts_total=retry_attempts_total,
            write_attempts_total=write_attempts_total,
            write_failures_total=write_failures_total,
            write_successes_total=write_successes_total,
            encode_last_ms=encode_last_ms,
            last_failure_age_ms=last_failure_age_ms,
        )

        diagnose_display_output_snapshot.additional_properties = d
        return diagnose_display_output_snapshot

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
