from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="ScreenCaptureCapacityStatus")


@_attrs_define
class ScreenCaptureCapacityStatus:
    """Installed byte fences for transactional screen publication admission.

    Attributes:
        admission_enforced (bool):
        analysis_height (int | None | Unset):
        analysis_parallel_capacity_per_second (int | None | Unset):
        analysis_peak_bytes (int | None | Unset):
        analysis_retained_bytes (int | None | Unset):
        analysis_serial_capacity_per_second (int | None | Unset):
        analysis_weighted_work_units_per_frame (int | None | Unset):
        analysis_weighted_work_units_per_second (int | None | Unset):
        analysis_width (int | None | Unset):
        analysis_worker_count (int | None | Unset):
        physical_available_bytes (int | None | Unset):
        physical_reserved_bytes (int | None | Unset):
        physical_transition_backend_capacity (int | None | Unset):
        physical_transition_byte_capacity (int | None | Unset):
        steady_publication_byte_budget (int | None | Unset):
        steady_total_backend_capacity (int | None | Unset):
        steady_total_byte_budget (int | None | Unset):
        transition_publication_backend_capacity (int | None | Unset):
    """

    admission_enforced: bool
    analysis_height: int | None | Unset = UNSET
    analysis_parallel_capacity_per_second: int | None | Unset = UNSET
    analysis_peak_bytes: int | None | Unset = UNSET
    analysis_retained_bytes: int | None | Unset = UNSET
    analysis_serial_capacity_per_second: int | None | Unset = UNSET
    analysis_weighted_work_units_per_frame: int | None | Unset = UNSET
    analysis_weighted_work_units_per_second: int | None | Unset = UNSET
    analysis_width: int | None | Unset = UNSET
    analysis_worker_count: int | None | Unset = UNSET
    physical_available_bytes: int | None | Unset = UNSET
    physical_reserved_bytes: int | None | Unset = UNSET
    physical_transition_backend_capacity: int | None | Unset = UNSET
    physical_transition_byte_capacity: int | None | Unset = UNSET
    steady_publication_byte_budget: int | None | Unset = UNSET
    steady_total_backend_capacity: int | None | Unset = UNSET
    steady_total_byte_budget: int | None | Unset = UNSET
    transition_publication_backend_capacity: int | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        admission_enforced = self.admission_enforced

        analysis_height: int | None | Unset
        if isinstance(self.analysis_height, Unset):
            analysis_height = UNSET
        else:
            analysis_height = self.analysis_height

        analysis_parallel_capacity_per_second: int | None | Unset
        if isinstance(self.analysis_parallel_capacity_per_second, Unset):
            analysis_parallel_capacity_per_second = UNSET
        else:
            analysis_parallel_capacity_per_second = (
                self.analysis_parallel_capacity_per_second
            )

        analysis_peak_bytes: int | None | Unset
        if isinstance(self.analysis_peak_bytes, Unset):
            analysis_peak_bytes = UNSET
        else:
            analysis_peak_bytes = self.analysis_peak_bytes

        analysis_retained_bytes: int | None | Unset
        if isinstance(self.analysis_retained_bytes, Unset):
            analysis_retained_bytes = UNSET
        else:
            analysis_retained_bytes = self.analysis_retained_bytes

        analysis_serial_capacity_per_second: int | None | Unset
        if isinstance(self.analysis_serial_capacity_per_second, Unset):
            analysis_serial_capacity_per_second = UNSET
        else:
            analysis_serial_capacity_per_second = (
                self.analysis_serial_capacity_per_second
            )

        analysis_weighted_work_units_per_frame: int | None | Unset
        if isinstance(self.analysis_weighted_work_units_per_frame, Unset):
            analysis_weighted_work_units_per_frame = UNSET
        else:
            analysis_weighted_work_units_per_frame = (
                self.analysis_weighted_work_units_per_frame
            )

        analysis_weighted_work_units_per_second: int | None | Unset
        if isinstance(self.analysis_weighted_work_units_per_second, Unset):
            analysis_weighted_work_units_per_second = UNSET
        else:
            analysis_weighted_work_units_per_second = (
                self.analysis_weighted_work_units_per_second
            )

        analysis_width: int | None | Unset
        if isinstance(self.analysis_width, Unset):
            analysis_width = UNSET
        else:
            analysis_width = self.analysis_width

        analysis_worker_count: int | None | Unset
        if isinstance(self.analysis_worker_count, Unset):
            analysis_worker_count = UNSET
        else:
            analysis_worker_count = self.analysis_worker_count

        physical_available_bytes: int | None | Unset
        if isinstance(self.physical_available_bytes, Unset):
            physical_available_bytes = UNSET
        else:
            physical_available_bytes = self.physical_available_bytes

        physical_reserved_bytes: int | None | Unset
        if isinstance(self.physical_reserved_bytes, Unset):
            physical_reserved_bytes = UNSET
        else:
            physical_reserved_bytes = self.physical_reserved_bytes

        physical_transition_backend_capacity: int | None | Unset
        if isinstance(self.physical_transition_backend_capacity, Unset):
            physical_transition_backend_capacity = UNSET
        else:
            physical_transition_backend_capacity = (
                self.physical_transition_backend_capacity
            )

        physical_transition_byte_capacity: int | None | Unset
        if isinstance(self.physical_transition_byte_capacity, Unset):
            physical_transition_byte_capacity = UNSET
        else:
            physical_transition_byte_capacity = self.physical_transition_byte_capacity

        steady_publication_byte_budget: int | None | Unset
        if isinstance(self.steady_publication_byte_budget, Unset):
            steady_publication_byte_budget = UNSET
        else:
            steady_publication_byte_budget = self.steady_publication_byte_budget

        steady_total_backend_capacity: int | None | Unset
        if isinstance(self.steady_total_backend_capacity, Unset):
            steady_total_backend_capacity = UNSET
        else:
            steady_total_backend_capacity = self.steady_total_backend_capacity

        steady_total_byte_budget: int | None | Unset
        if isinstance(self.steady_total_byte_budget, Unset):
            steady_total_byte_budget = UNSET
        else:
            steady_total_byte_budget = self.steady_total_byte_budget

        transition_publication_backend_capacity: int | None | Unset
        if isinstance(self.transition_publication_backend_capacity, Unset):
            transition_publication_backend_capacity = UNSET
        else:
            transition_publication_backend_capacity = (
                self.transition_publication_backend_capacity
            )

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "admission_enforced": admission_enforced,
            }
        )
        if analysis_height is not UNSET:
            field_dict["analysis_height"] = analysis_height
        if analysis_parallel_capacity_per_second is not UNSET:
            field_dict["analysis_parallel_capacity_per_second"] = (
                analysis_parallel_capacity_per_second
            )
        if analysis_peak_bytes is not UNSET:
            field_dict["analysis_peak_bytes"] = analysis_peak_bytes
        if analysis_retained_bytes is not UNSET:
            field_dict["analysis_retained_bytes"] = analysis_retained_bytes
        if analysis_serial_capacity_per_second is not UNSET:
            field_dict["analysis_serial_capacity_per_second"] = (
                analysis_serial_capacity_per_second
            )
        if analysis_weighted_work_units_per_frame is not UNSET:
            field_dict["analysis_weighted_work_units_per_frame"] = (
                analysis_weighted_work_units_per_frame
            )
        if analysis_weighted_work_units_per_second is not UNSET:
            field_dict["analysis_weighted_work_units_per_second"] = (
                analysis_weighted_work_units_per_second
            )
        if analysis_width is not UNSET:
            field_dict["analysis_width"] = analysis_width
        if analysis_worker_count is not UNSET:
            field_dict["analysis_worker_count"] = analysis_worker_count
        if physical_available_bytes is not UNSET:
            field_dict["physical_available_bytes"] = physical_available_bytes
        if physical_reserved_bytes is not UNSET:
            field_dict["physical_reserved_bytes"] = physical_reserved_bytes
        if physical_transition_backend_capacity is not UNSET:
            field_dict["physical_transition_backend_capacity"] = (
                physical_transition_backend_capacity
            )
        if physical_transition_byte_capacity is not UNSET:
            field_dict["physical_transition_byte_capacity"] = (
                physical_transition_byte_capacity
            )
        if steady_publication_byte_budget is not UNSET:
            field_dict["steady_publication_byte_budget"] = (
                steady_publication_byte_budget
            )
        if steady_total_backend_capacity is not UNSET:
            field_dict["steady_total_backend_capacity"] = steady_total_backend_capacity
        if steady_total_byte_budget is not UNSET:
            field_dict["steady_total_byte_budget"] = steady_total_byte_budget
        if transition_publication_backend_capacity is not UNSET:
            field_dict["transition_publication_backend_capacity"] = (
                transition_publication_backend_capacity
            )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        admission_enforced = d.pop("admission_enforced")

        def _parse_analysis_height(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        analysis_height = _parse_analysis_height(d.pop("analysis_height", UNSET))

        def _parse_analysis_parallel_capacity_per_second(
            data: object,
        ) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        analysis_parallel_capacity_per_second = (
            _parse_analysis_parallel_capacity_per_second(
                d.pop("analysis_parallel_capacity_per_second", UNSET)
            )
        )

        def _parse_analysis_peak_bytes(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        analysis_peak_bytes = _parse_analysis_peak_bytes(
            d.pop("analysis_peak_bytes", UNSET)
        )

        def _parse_analysis_retained_bytes(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        analysis_retained_bytes = _parse_analysis_retained_bytes(
            d.pop("analysis_retained_bytes", UNSET)
        )

        def _parse_analysis_serial_capacity_per_second(
            data: object,
        ) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        analysis_serial_capacity_per_second = (
            _parse_analysis_serial_capacity_per_second(
                d.pop("analysis_serial_capacity_per_second", UNSET)
            )
        )

        def _parse_analysis_weighted_work_units_per_frame(
            data: object,
        ) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        analysis_weighted_work_units_per_frame = (
            _parse_analysis_weighted_work_units_per_frame(
                d.pop("analysis_weighted_work_units_per_frame", UNSET)
            )
        )

        def _parse_analysis_weighted_work_units_per_second(
            data: object,
        ) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        analysis_weighted_work_units_per_second = (
            _parse_analysis_weighted_work_units_per_second(
                d.pop("analysis_weighted_work_units_per_second", UNSET)
            )
        )

        def _parse_analysis_width(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        analysis_width = _parse_analysis_width(d.pop("analysis_width", UNSET))

        def _parse_analysis_worker_count(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        analysis_worker_count = _parse_analysis_worker_count(
            d.pop("analysis_worker_count", UNSET)
        )

        def _parse_physical_available_bytes(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        physical_available_bytes = _parse_physical_available_bytes(
            d.pop("physical_available_bytes", UNSET)
        )

        def _parse_physical_reserved_bytes(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        physical_reserved_bytes = _parse_physical_reserved_bytes(
            d.pop("physical_reserved_bytes", UNSET)
        )

        def _parse_physical_transition_backend_capacity(
            data: object,
        ) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        physical_transition_backend_capacity = (
            _parse_physical_transition_backend_capacity(
                d.pop("physical_transition_backend_capacity", UNSET)
            )
        )

        def _parse_physical_transition_byte_capacity(
            data: object,
        ) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        physical_transition_byte_capacity = _parse_physical_transition_byte_capacity(
            d.pop("physical_transition_byte_capacity", UNSET)
        )

        def _parse_steady_publication_byte_budget(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        steady_publication_byte_budget = _parse_steady_publication_byte_budget(
            d.pop("steady_publication_byte_budget", UNSET)
        )

        def _parse_steady_total_backend_capacity(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        steady_total_backend_capacity = _parse_steady_total_backend_capacity(
            d.pop("steady_total_backend_capacity", UNSET)
        )

        def _parse_steady_total_byte_budget(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        steady_total_byte_budget = _parse_steady_total_byte_budget(
            d.pop("steady_total_byte_budget", UNSET)
        )

        def _parse_transition_publication_backend_capacity(
            data: object,
        ) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        transition_publication_backend_capacity = (
            _parse_transition_publication_backend_capacity(
                d.pop("transition_publication_backend_capacity", UNSET)
            )
        )

        screen_capture_capacity_status = cls(
            admission_enforced=admission_enforced,
            analysis_height=analysis_height,
            analysis_parallel_capacity_per_second=analysis_parallel_capacity_per_second,
            analysis_peak_bytes=analysis_peak_bytes,
            analysis_retained_bytes=analysis_retained_bytes,
            analysis_serial_capacity_per_second=analysis_serial_capacity_per_second,
            analysis_weighted_work_units_per_frame=analysis_weighted_work_units_per_frame,
            analysis_weighted_work_units_per_second=analysis_weighted_work_units_per_second,
            analysis_width=analysis_width,
            analysis_worker_count=analysis_worker_count,
            physical_available_bytes=physical_available_bytes,
            physical_reserved_bytes=physical_reserved_bytes,
            physical_transition_backend_capacity=physical_transition_backend_capacity,
            physical_transition_byte_capacity=physical_transition_byte_capacity,
            steady_publication_byte_budget=steady_publication_byte_budget,
            steady_total_backend_capacity=steady_total_backend_capacity,
            steady_total_byte_budget=steady_total_byte_budget,
            transition_publication_backend_capacity=transition_publication_backend_capacity,
        )

        screen_capture_capacity_status.additional_properties = d
        return screen_capture_capacity_status

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
