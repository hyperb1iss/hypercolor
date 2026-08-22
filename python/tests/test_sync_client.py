"""Tests for the sync client wrapper."""

from __future__ import annotations

import json

import httpx
import msgspec

from hypercolor._generated.types import Unset
from hypercolor.models import EffectPresetOrigin
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
                    "checks": {
                        "render_loop": "ok",
                        "device_backends": "ok",
                        "event_bus": "ok",
                    },
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


def test_sync_client_preserves_included_device_attachments() -> None:
    def handler(request: httpx.Request) -> httpx.Response:
        assert request.url.path == "/api/v1/devices"
        assert request.url.params["include"] == "attachments"
        return httpx.Response(
            200,
            json={
                "data": {
                    "items": [
                        {
                            "id": "controller",
                            "layout_device_id": "controller",
                            "name": "Controller",
                            "origin": {
                                "driver_id": "razer",
                                "transport": "usb",
                            },
                            "status": "connected",
                            "brightness": 100,
                            "total_leds": 60,
                            "segments": [],
                            "attachments": {
                                "device_id": "controller",
                                "device_name": "Controller",
                                "slots": [
                                    {
                                        "id": "channel-1",
                                        "name": "Channel 1",
                                        "led_start": 0,
                                        "led_count": 60,
                                        "allowed_templates": ["strip-60"],
                                    }
                                ],
                                "bindings": [
                                    {
                                        "slot_id": "channel-1",
                                        "template_id": "strip-60",
                                        "template_name": "60 LED Strip",
                                        "enabled": True,
                                        "instances": 1,
                                        "led_offset": 0,
                                        "effective_led_count": 60,
                                    }
                                ],
                                "suggested_zones": [],
                            },
                        }
                    ],
                    "total": 1,
                    "page": {
                        "offset": 0,
                        "limit": 50,
                        "has_more": False,
                    },
                },
                "meta": {
                    "api_version": "1.0",
                    "request_id": "req_123",
                    "timestamp": "2026-08-19T00:00:00Z",
                },
            },
        )

    client = SyncHypercolorClient(transport=httpx.MockTransport(handler))
    try:
        devices = client.get_devices(include="attachments")
    finally:
        client.close()

    attachments = devices[0].attachments
    assert attachments is not None
    assert attachments.slots[0].allowed_templates == ["strip-60"]
    assert attachments.bindings[0].template_name == "60 LED Strip"


def test_sync_client_round_trips_complete_stored_scene() -> None:
    scene = {
        "id": "0193d2c0-0000-7000-8000-00000000aaaa",
        "name": "Battlestation",
        "description": "Daily desk scene",
        "kind": "named",
        "is_default": False,
        "unassigned_behavior": "off",
        "layout_id": None,
        "activation_brightness": 0.75,
        "transition": {
            "duration_ms": 1000,
            "easing": "Linear",
            "color_interpolation": "Oklab",
        },
        "priority": 50,
        "enabled": True,
        "metadata": {"room": "office"},
        "mutation_mode": "live",
        "revision": 12,
        "zones": [],
    }

    def handler(request: httpx.Request) -> httpx.Response:
        assert request.url.path == f"/api/v1/scenes/{scene['id']}"
        if request.method == "PUT":
            assert request.headers["if-match"] == '"12"'
            assert json.loads(request.content) == {
                key: value for key, value in scene.items() if key not in {"is_default", "revision"}
            }
        return httpx.Response(
            200,
            content=msgspec.json.encode(
                {
                    "data": scene,
                    "meta": {
                        "api_version": "1.0",
                        "request_id": "req_scene",
                        "timestamp": "2026-08-19T00:00:00Z",
                    },
                }
            ),
        )

    client = SyncHypercolorClient(transport=httpx.MockTransport(handler))
    try:
        document = client.get_scene(scene["id"])
        updated = client.update_scene(scene["id"], document, if_match=document.revision)
    finally:
        client.close()

    assert not isinstance(document.metadata, Unset)
    assert document.metadata.to_dict() == {"room": "office"}
    assert updated.activation_brightness == 0.75


def test_sync_client_snapshots_the_live_scene() -> None:
    def handler(request: httpx.Request) -> httpx.Response:
        assert request.url.path == "/api/v1/scenes/snapshot"
        assert request.method == "POST"
        assert json.loads(request.content) == {
            "name": "Evening",
            "description": "Soft desk lighting",
        }
        return httpx.Response(
            201,
            content=msgspec.json.encode(
                {
                    "data": {
                        "id": "scene-snapshot",
                        "name": "Evening",
                        "description": "Soft desk lighting",
                        "enabled": True,
                        "priority": 50,
                        "mutation_mode": "snapshot",
                    },
                    "meta": {
                        "api_version": "1.0",
                        "request_id": "req_scene",
                        "timestamp": "2026-08-19T00:00:00Z",
                    },
                }
            ),
        )

    client = SyncHypercolorClient(transport=httpx.MockTransport(handler))
    try:
        scene = client.snapshot_scene("Evening", description="Soft desk lighting")
    finally:
        client.close()

    assert scene.mutation_mode == "snapshot"


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
            "values": {"enabled": {"kind": "bool", "value": True}},
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
    assert result.values["enabled"] == {"kind": "bool", "value": True}


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
                "total": 1,
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
                            "transform": {
                                "anchor": {"x": 0.5, "y": 0.5},
                                "scale": [1.0, 1.0],
                                "rotation": 0.0,
                                "fit": "cover",
                            },
                            "adjust": {
                                "brightness": 1.0,
                                "saturation": 1.0,
                                "hue_shift": 0.0,
                                "tint": [1.0, 1.0, 1.0, 1.0],
                                "tint_strength": 0.0,
                                "contrast": 0.0,
                            },
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
