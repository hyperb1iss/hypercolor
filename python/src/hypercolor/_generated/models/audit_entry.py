from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.audit_transport import AuditTransport
from ..types import UNSET, Unset

T = TypeVar("T", bound="AuditEntry")


@_attrs_define
class AuditEntry:
    """One state-changing request, as the daemon recorded it.

    Entries never carry request bodies, query strings, headers other than
    the user agent, or tool arguments.

        Attributes:
            latency_ms (float): Handling time in milliseconds.
            method (str): HTTP method, or `tools/call` for MCP.
            path (str): Request path without its query string, or the MCP mount path.
            remote (str): Client address, or `unknown` when the transport has none.
            status (int): Response status. MCP tool calls report 200 for success and the
                closest HTTP status for a tool error (400, 404, 409, or 500).
            timestamp (str): When the request finished, RFC 3339 UTC with milliseconds.
            transport (AuditTransport): Transport a state-changing request arrived on.
            user_agent (str): Client `User-Agent`, empty when absent.
            stores (list[str] | Unset): Durable stores whose bytes this request changed, by inventory name.
            tool (None | str | Unset): MCP tool name.
    """

    latency_ms: float
    method: str
    path: str
    remote: str
    status: int
    timestamp: str
    transport: AuditTransport
    user_agent: str
    stores: list[str] | Unset = UNSET
    tool: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        latency_ms = self.latency_ms

        method = self.method

        path = self.path

        remote = self.remote

        status = self.status

        timestamp = self.timestamp

        transport = self.transport.value

        user_agent = self.user_agent

        stores: list[str] | Unset = UNSET
        if not isinstance(self.stores, Unset):
            stores = self.stores

        tool: None | str | Unset
        if isinstance(self.tool, Unset):
            tool = UNSET
        else:
            tool = self.tool

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "latency_ms": latency_ms,
                "method": method,
                "path": path,
                "remote": remote,
                "status": status,
                "timestamp": timestamp,
                "transport": transport,
                "user_agent": user_agent,
            }
        )
        if stores is not UNSET:
            field_dict["stores"] = stores
        if tool is not UNSET:
            field_dict["tool"] = tool

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        latency_ms = d.pop("latency_ms")

        method = d.pop("method")

        path = d.pop("path")

        remote = d.pop("remote")

        status = d.pop("status")

        timestamp = d.pop("timestamp")

        transport = AuditTransport(d.pop("transport"))

        user_agent = d.pop("user_agent")

        stores = cast(list[str], d.pop("stores", UNSET))

        def _parse_tool(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        tool = _parse_tool(d.pop("tool", UNSET))

        audit_entry = cls(
            latency_ms=latency_ms,
            method=method,
            path=path,
            remote=remote,
            status=status,
            timestamp=timestamp,
            transport=transport,
            user_agent=user_agent,
            stores=stores,
            tool=tool,
        )

        audit_entry.additional_properties = d
        return audit_entry

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
