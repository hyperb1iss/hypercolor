from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..types import UNSET, Unset

T = TypeVar("T", bound="RenderFailureStatus")


@_attrs_define
class RenderFailureStatus:
    """One render failure, as the supervisor recorded it.

    Attributes:
        action (str): `restart`, `terminate_process`, `daemon_stopping`, `process_abort`,
            or `awaiting_supervisor`.
        current_process (bool): Whether this daemon run recorded the failure, as opposed to an
            earlier run whose record survived in the state directory.
        kind (str): `panic`, `input_pump_terminated`, `input_pump_restart_failed`,
            `pipeline_rebuild_failed`, or `gpu_device_lost`.
        message (str): The panic message or error chain. Empty when the caller may not
            read private diagnostics.
        occurred_at (str): RFC 3339 UTC time of the failure.
        restart_attempt (int): Restarts counted in the budget window by this failure.
        location (None | str | Unset):
        thread (None | str | Unset):
    """

    action: str
    current_process: bool
    kind: str
    message: str
    occurred_at: str
    restart_attempt: int
    location: None | str | Unset = UNSET
    thread: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        action = self.action

        current_process = self.current_process

        kind = self.kind

        message = self.message

        occurred_at = self.occurred_at

        restart_attempt = self.restart_attempt

        location: None | str | Unset
        if isinstance(self.location, Unset):
            location = UNSET
        else:
            location = self.location

        thread: None | str | Unset
        if isinstance(self.thread, Unset):
            thread = UNSET
        else:
            thread = self.thread

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "action": action,
                "current_process": current_process,
                "kind": kind,
                "message": message,
                "occurred_at": occurred_at,
                "restart_attempt": restart_attempt,
            }
        )
        if location is not UNSET:
            field_dict["location"] = location
        if thread is not UNSET:
            field_dict["thread"] = thread

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        action = d.pop("action")

        current_process = d.pop("current_process")

        kind = d.pop("kind")

        message = d.pop("message")

        occurred_at = d.pop("occurred_at")

        restart_attempt = d.pop("restart_attempt")

        def _parse_location(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        location = _parse_location(d.pop("location", UNSET))

        def _parse_thread(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        thread = _parse_thread(d.pop("thread", UNSET))

        render_failure_status = cls(
            action=action,
            current_process=current_process,
            kind=kind,
            message=message,
            occurred_at=occurred_at,
            restart_attempt=restart_attempt,
            location=location,
            thread=thread,
        )

        render_failure_status.additional_properties = d
        return render_failure_status

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
