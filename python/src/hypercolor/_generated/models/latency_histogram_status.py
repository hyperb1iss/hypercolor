from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.latency_histogram_bucket_status import LatencyHistogramBucketStatus


T = TypeVar("T", bound="LatencyHistogramStatus")


@_attrs_define
class LatencyHistogramStatus:
    """
    Attributes:
        bucket_width_us (int):
        buckets (list[LatencyHistogramBucketStatus]):
        overflow_bucket_index (int):
        snapshot_frame_token (int | None | Unset):
    """

    bucket_width_us: int
    buckets: list[LatencyHistogramBucketStatus]
    overflow_bucket_index: int
    snapshot_frame_token: int | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        bucket_width_us = self.bucket_width_us

        buckets = []
        for buckets_item_data in self.buckets:
            buckets_item = buckets_item_data.to_dict()
            buckets.append(buckets_item)

        overflow_bucket_index = self.overflow_bucket_index

        snapshot_frame_token: int | None | Unset
        if isinstance(self.snapshot_frame_token, Unset):
            snapshot_frame_token = UNSET
        else:
            snapshot_frame_token = self.snapshot_frame_token

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "bucket_width_us": bucket_width_us,
                "buckets": buckets,
                "overflow_bucket_index": overflow_bucket_index,
            }
        )
        if snapshot_frame_token is not UNSET:
            field_dict["snapshot_frame_token"] = snapshot_frame_token

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.latency_histogram_bucket_status import (
            LatencyHistogramBucketStatus,
        )

        d = dict(src_dict)
        bucket_width_us = d.pop("bucket_width_us")

        buckets = []
        _buckets = d.pop("buckets")
        for buckets_item_data in _buckets:
            buckets_item = LatencyHistogramBucketStatus.from_dict(buckets_item_data)

            buckets.append(buckets_item)

        overflow_bucket_index = d.pop("overflow_bucket_index")

        def _parse_snapshot_frame_token(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        snapshot_frame_token = _parse_snapshot_frame_token(
            d.pop("snapshot_frame_token", UNSET)
        )

        latency_histogram_status = cls(
            bucket_width_us=bucket_width_us,
            buckets=buckets,
            overflow_bucket_index=overflow_bucket_index,
            snapshot_frame_token=snapshot_frame_token,
        )

        latency_histogram_status.additional_properties = d
        return latency_histogram_status

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
