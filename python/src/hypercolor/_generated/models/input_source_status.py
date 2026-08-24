from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.input_source_issue_status import InputSourceIssueStatus
    from ..models.source_diagnostics_envelope import SourceDiagnosticsEnvelope


T = TypeVar("T", bound="InputSourceStatus")


@_attrs_define
class InputSourceStatus:
    """Lock-free lifecycle and freshness status for one input source.

    Attributes:
        active_consumer_count (int):
        backend (str):
        configured (bool):
        consented (bool):
        demanded (bool):
        denied_resource_count (int):
        freshness (str):
        kind (str):
        resource_count (int):
        retired (bool):
        session_generation (int):
        source_graph_generation (int):
        source_id (str):
        state (str):
        action_issue (InputSourceIssueStatus | None | Unset):
        diagnostics (None | SourceDiagnosticsEnvelope | Unset):
        freshness_issue (InputSourceIssueStatus | None | Unset):
        freshness_remaining_ms (int | None | Unset):
        issue (InputSourceIssueStatus | None | Unset):
        last_sample_age_ms (int | None | Unset):
        lifecycle_issue (InputSourceIssueStatus | None | Unset):
    """

    active_consumer_count: int
    backend: str
    configured: bool
    consented: bool
    demanded: bool
    denied_resource_count: int
    freshness: str
    kind: str
    resource_count: int
    retired: bool
    session_generation: int
    source_graph_generation: int
    source_id: str
    state: str
    action_issue: InputSourceIssueStatus | None | Unset = UNSET
    diagnostics: None | SourceDiagnosticsEnvelope | Unset = UNSET
    freshness_issue: InputSourceIssueStatus | None | Unset = UNSET
    freshness_remaining_ms: int | None | Unset = UNSET
    issue: InputSourceIssueStatus | None | Unset = UNSET
    last_sample_age_ms: int | None | Unset = UNSET
    lifecycle_issue: InputSourceIssueStatus | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.input_source_issue_status import InputSourceIssueStatus
        from ..models.source_diagnostics_envelope import SourceDiagnosticsEnvelope

        active_consumer_count = self.active_consumer_count

        backend = self.backend

        configured = self.configured

        consented = self.consented

        demanded = self.demanded

        denied_resource_count = self.denied_resource_count

        freshness = self.freshness

        kind = self.kind

        resource_count = self.resource_count

        retired = self.retired

        session_generation = self.session_generation

        source_graph_generation = self.source_graph_generation

        source_id = self.source_id

        state = self.state

        action_issue: dict[str, Any] | None | Unset
        if isinstance(self.action_issue, Unset):
            action_issue = UNSET
        elif isinstance(self.action_issue, InputSourceIssueStatus):
            action_issue = self.action_issue.to_dict()
        else:
            action_issue = self.action_issue

        diagnostics: dict[str, Any] | None | Unset
        if isinstance(self.diagnostics, Unset):
            diagnostics = UNSET
        elif isinstance(self.diagnostics, SourceDiagnosticsEnvelope):
            diagnostics = self.diagnostics.to_dict()
        else:
            diagnostics = self.diagnostics

        freshness_issue: dict[str, Any] | None | Unset
        if isinstance(self.freshness_issue, Unset):
            freshness_issue = UNSET
        elif isinstance(self.freshness_issue, InputSourceIssueStatus):
            freshness_issue = self.freshness_issue.to_dict()
        else:
            freshness_issue = self.freshness_issue

        freshness_remaining_ms: int | None | Unset
        if isinstance(self.freshness_remaining_ms, Unset):
            freshness_remaining_ms = UNSET
        else:
            freshness_remaining_ms = self.freshness_remaining_ms

        issue: dict[str, Any] | None | Unset
        if isinstance(self.issue, Unset):
            issue = UNSET
        elif isinstance(self.issue, InputSourceIssueStatus):
            issue = self.issue.to_dict()
        else:
            issue = self.issue

        last_sample_age_ms: int | None | Unset
        if isinstance(self.last_sample_age_ms, Unset):
            last_sample_age_ms = UNSET
        else:
            last_sample_age_ms = self.last_sample_age_ms

        lifecycle_issue: dict[str, Any] | None | Unset
        if isinstance(self.lifecycle_issue, Unset):
            lifecycle_issue = UNSET
        elif isinstance(self.lifecycle_issue, InputSourceIssueStatus):
            lifecycle_issue = self.lifecycle_issue.to_dict()
        else:
            lifecycle_issue = self.lifecycle_issue

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "active_consumer_count": active_consumer_count,
                "backend": backend,
                "configured": configured,
                "consented": consented,
                "demanded": demanded,
                "denied_resource_count": denied_resource_count,
                "freshness": freshness,
                "kind": kind,
                "resource_count": resource_count,
                "retired": retired,
                "session_generation": session_generation,
                "source_graph_generation": source_graph_generation,
                "source_id": source_id,
                "state": state,
            }
        )
        if action_issue is not UNSET:
            field_dict["action_issue"] = action_issue
        if diagnostics is not UNSET:
            field_dict["diagnostics"] = diagnostics
        if freshness_issue is not UNSET:
            field_dict["freshness_issue"] = freshness_issue
        if freshness_remaining_ms is not UNSET:
            field_dict["freshness_remaining_ms"] = freshness_remaining_ms
        if issue is not UNSET:
            field_dict["issue"] = issue
        if last_sample_age_ms is not UNSET:
            field_dict["last_sample_age_ms"] = last_sample_age_ms
        if lifecycle_issue is not UNSET:
            field_dict["lifecycle_issue"] = lifecycle_issue

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.input_source_issue_status import InputSourceIssueStatus
        from ..models.source_diagnostics_envelope import SourceDiagnosticsEnvelope

        d = dict(src_dict)
        active_consumer_count = d.pop("active_consumer_count")

        backend = d.pop("backend")

        configured = d.pop("configured")

        consented = d.pop("consented")

        demanded = d.pop("demanded")

        denied_resource_count = d.pop("denied_resource_count")

        freshness = d.pop("freshness")

        kind = d.pop("kind")

        resource_count = d.pop("resource_count")

        retired = d.pop("retired")

        session_generation = d.pop("session_generation")

        source_graph_generation = d.pop("source_graph_generation")

        source_id = d.pop("source_id")

        state = d.pop("state")

        def _parse_action_issue(data: object) -> InputSourceIssueStatus | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                action_issue_type_1 = InputSourceIssueStatus.from_dict(data)

                return action_issue_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(InputSourceIssueStatus | None | Unset, data)

        action_issue = _parse_action_issue(d.pop("action_issue", UNSET))

        def _parse_diagnostics(
            data: object,
        ) -> None | SourceDiagnosticsEnvelope | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                diagnostics_type_1 = SourceDiagnosticsEnvelope.from_dict(data)

                return diagnostics_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | SourceDiagnosticsEnvelope | Unset, data)

        diagnostics = _parse_diagnostics(d.pop("diagnostics", UNSET))

        def _parse_freshness_issue(
            data: object,
        ) -> InputSourceIssueStatus | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                freshness_issue_type_1 = InputSourceIssueStatus.from_dict(data)

                return freshness_issue_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(InputSourceIssueStatus | None | Unset, data)

        freshness_issue = _parse_freshness_issue(d.pop("freshness_issue", UNSET))

        def _parse_freshness_remaining_ms(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        freshness_remaining_ms = _parse_freshness_remaining_ms(
            d.pop("freshness_remaining_ms", UNSET)
        )

        def _parse_issue(data: object) -> InputSourceIssueStatus | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                issue_type_1 = InputSourceIssueStatus.from_dict(data)

                return issue_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(InputSourceIssueStatus | None | Unset, data)

        issue = _parse_issue(d.pop("issue", UNSET))

        def _parse_last_sample_age_ms(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        last_sample_age_ms = _parse_last_sample_age_ms(
            d.pop("last_sample_age_ms", UNSET)
        )

        def _parse_lifecycle_issue(
            data: object,
        ) -> InputSourceIssueStatus | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                lifecycle_issue_type_1 = InputSourceIssueStatus.from_dict(data)

                return lifecycle_issue_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(InputSourceIssueStatus | None | Unset, data)

        lifecycle_issue = _parse_lifecycle_issue(d.pop("lifecycle_issue", UNSET))

        input_source_status = cls(
            active_consumer_count=active_consumer_count,
            backend=backend,
            configured=configured,
            consented=consented,
            demanded=demanded,
            denied_resource_count=denied_resource_count,
            freshness=freshness,
            kind=kind,
            resource_count=resource_count,
            retired=retired,
            session_generation=session_generation,
            source_graph_generation=source_graph_generation,
            source_id=source_id,
            state=state,
            action_issue=action_issue,
            diagnostics=diagnostics,
            freshness_issue=freshness_issue,
            freshness_remaining_ms=freshness_remaining_ms,
            issue=issue,
            last_sample_age_ms=last_sample_age_ms,
            lifecycle_issue=lifecycle_issue,
        )

        input_source_status.additional_properties = d
        return input_source_status

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
