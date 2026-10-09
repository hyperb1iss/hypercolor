from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..models.daemon_startup_phase import DaemonStartupPhase
from ..types import UNSET, Unset

T = TypeVar("T", bound="DaemonStartupProgress")


@_attrs_define
class DaemonStartupProgress:
    """How far a starting daemon has come, reported by `/health` before the
    full API is served.

        Attributes:
            phase (DaemonStartupPhase): Coarse daemon startup phases, in the order the daemon enters them.
            sequence (int): Monotonic counter that advances every time startup completes a unit
                of work: entering a phase, or finishing a step inside one (such as
                compiling one compositor pipeline). A client compares successive
                values to tell a slow startup from a stuck one.
            detail (None | str | Unset): The step inside the phase that is running right now, when startup
                reports one. It names the work that has not finished yet, so a
                startup that stops advancing points at the step it is stuck in.
    """

    phase: DaemonStartupPhase
    sequence: int
    detail: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        phase = self.phase.value

        sequence = self.sequence

        detail: None | str | Unset
        if isinstance(self.detail, Unset):
            detail = UNSET
        else:
            detail = self.detail

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "phase": phase,
                "sequence": sequence,
            }
        )
        if detail is not UNSET:
            field_dict["detail"] = detail

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        phase = DaemonStartupPhase(d.pop("phase"))

        sequence = d.pop("sequence")

        def _parse_detail(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        detail = _parse_detail(d.pop("detail", UNSET))

        daemon_startup_progress = cls(
            phase=phase,
            sequence=sequence,
            detail=detail,
        )

        daemon_startup_progress.additional_properties = d
        return daemon_startup_progress

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
