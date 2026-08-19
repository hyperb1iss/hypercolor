"""Tests for the sync client wrapper."""

from __future__ import annotations

import json

import httpx
import msgspec

from hypercolor.models.effect import EffectPresetOrigin
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


def test_sync_client_delegates_output_power() -> None:
    def handler(request: httpx.Request) -> httpx.Response:
        assert request.url.path == "/api/v1/output"
        assert request.method == "PATCH"
        assert json.loads(request.content) == {"power": "paused"}
        return httpx.Response(
            200,
            content=msgspec.json.encode(
                {
                    "data": {"power": "paused", "brightness": 0.8},
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
        result = client.pause_rendering()
    finally:
        client.close()

    assert result.paused is True
    assert result.brightness_percent == 80


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


def test_sync_client_delegates_effect_preset_stack() -> None:
    def handler(request: httpx.Request) -> httpx.Response:
        if request.method == "GET":
            assert request.url.raw_path == b"/api/v1/effects/aurora%2Fmain/presets"
            data: object = {
                "items": [
                    {
                        "id": "saved-bright",
                        "name": "Bright",
                        "description": None,
                        "effect_id": "aurora/main",
                        "controls": {},
                        "tags": [],
                        "origin": "saved",
                        "editable": True,
                    }
                ],
                "pagination": {"offset": 0, "limit": 1, "total": 1, "has_more": False},
            }
        else:
            expected_prefix = b"/api/v1/effects/aurora%2Fmain/presets/"
            assert request.url.raw_path.startswith(expected_prefix)
            if request.url.raw_path.endswith(b"/saved-bright/apply"):
                assert json.loads(request.content) == {"zone": "zone-left"}
                assert request.headers["if-match"] == '"17"'
            else:
                assert request.url.raw_path.endswith(b"/bundled-calm/apply")
                assert request.content == b""
                assert "if-match" not in request.headers
            data = {
                "zone": {
                    "id": "0193d2c0-0000-7000-8000-000000000001",
                    "name": "Primary",
                    "role": "primary",
                    "enabled": True,
                    "brightness": 1.0,
                    "color": None,
                    "display_target": None,
                    "members": [],
                    "layout": None,
                    "layers": [
                        {
                            "id": "0193d2c0-0000-7000-8000-000000000002",
                            "source": {
                                "type": "effect",
                                "effect_id": "aurora/main",
                                "controls": {},
                                "control_bindings": {},
                                "preset_id": None,
                            },
                            "blend": "replace",
                            "opacity": 1.0,
                            "transform": {},
                            "adjust": {},
                            "enabled": True,
                        }
                    ],
                },
                "transition": {"type": "cut"},
                "output": {"applied": True},
            }
        return httpx.Response(
            200,
            content=msgspec.json.encode(
                {
                    "data": data,
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
        presets = client.get_effect_presets("aurora/main")
        result = client.apply_effect_preset(
            "aurora/main",
            "saved-bright",
            zone="zone-left",
            if_match=17,
        )
        ungrouped_result = client.apply_effect_preset("aurora/main", "bundled-calm")
    finally:
        client.close()

    assert presets[0].origin is EffectPresetOrigin.SAVED
    assert presets[0].editable is True
    assert result.zone.layers[0].source["effect_id"] == "aurora/main"
    assert ungrouped_result.zone.layers[0].source["effect_id"] == "aurora/main"
