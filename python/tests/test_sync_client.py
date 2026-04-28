"""Tests for the sync client wrapper."""

from __future__ import annotations

import json

import httpx
import msgspec

from hypercolor.sync_client import SyncHypercolorClient


def test_sync_client_delegates_health() -> None:
    def handler(request: httpx.Request) -> httpx.Response:
        assert request.url.path == "/health"
        return httpx.Response(
            200,
            content=msgspec.json.encode(
                {
                    "status": "healthy",
                    "version": "0.1.0",
                    "uptime_seconds": 42,
                    "checks": {"render_loop": "ok"},
                }
            ),
        )

    client = SyncHypercolorClient(transport=httpx.MockTransport(handler))
    try:
        result = client.health()
    finally:
        client.close()

    assert result.status == "healthy"


def test_sync_client_delegates_driver_inventory() -> None:
    def handler(request: httpx.Request) -> httpx.Response:
        assert request.url.path == "/api/v1/drivers"
        return httpx.Response(
            200,
            content=msgspec.json.encode(
                {
                    "data": {
                        "items": [
                            {
                                "descriptor": {
                                    "id": "nollie",
                                    "display_name": "Nollie",
                                    "module_kind": "hal",
                                    "transports": ["usb"],
                                    "capabilities": {
                                        "config": False,
                                        "discovery": True,
                                        "pairing": False,
                                        "output_backend": False,
                                        "protocol_catalog": True,
                                        "runtime_cache": False,
                                        "credentials": False,
                                        "presentation": True,
                                        "controls": False,
                                    },
                                    "api_schema_version": 1,
                                    "config_version": 1,
                                    "default_enabled": True,
                                },
                                "enabled": True,
                                "config_key": "drivers.nollie",
                                "protocols": [
                                    {
                                        "driver_id": "nollie",
                                        "protocol_id": "nollie_8",
                                        "display_name": "Nollie 8",
                                        "family_id": "nollie",
                                        "transport": "usb",
                                        "route_backend_id": "usb",
                                    }
                                ],
                            }
                        ]
                    },
                    "meta": {
                        "api_version": "1.0",
                        "request_id": "req_123",
                        "timestamp": "2026-03-08T00:00:00Z",
                    },
                }
            ),
        )

    client = SyncHypercolorClient(transport=httpx.MockTransport(handler))
    try:
        result = client.get_drivers()
    finally:
        client.close()

    assert result[0].descriptor.id == "nollie"
    assert result[0].protocols[0].route_backend_id == "usb"


def test_sync_client_delegates_control_values() -> None:
    def handler(request: httpx.Request) -> httpx.Response:
        assert request.url.raw_path == b"/api/v1/control-surfaces/device%3Akeyboard/values"
        assert json.loads(request.content) == {
            "surface_id": "device:keyboard",
            "changes": [{"field_id": "enabled", "value": {"kind": "bool", "value": True}}],
        }
        return httpx.Response(
            200,
            content=msgspec.json.encode(
                {
                    "data": {
                        "surface_id": "device:keyboard",
                        "previous_revision": 1,
                        "revision": 2,
                        "accepted": [],
                        "rejected": [],
                        "impacts": [],
                        "values": {"enabled": {"kind": "bool", "value": True}},
                    },
                    "meta": {
                        "api_version": "1.0",
                        "request_id": "req_123",
                        "timestamp": "2026-03-08T00:00:00Z",
                    },
                }
            ),
        )

    client = SyncHypercolorClient(transport=httpx.MockTransport(handler))
    try:
        result = client.set_control_values("device:keyboard", {"enabled": True})
    finally:
        client.close()

    assert result.revision == 2
    assert result.values["enabled"] is True
