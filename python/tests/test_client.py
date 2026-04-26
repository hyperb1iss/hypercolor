"""Tests for the async Hypercolor client."""

from __future__ import annotations

import json

import httpx
import msgspec
import pytest
import respx

from hypercolor.client import HypercolorClient
from hypercolor.exceptions import (
    HypercolorConnectionError,
    HypercolorNotFoundError,
    HypercolorValidationError,
)
from hypercolor.models.effect import ActiveEffect, Effect


def _envelope(data: object) -> bytes:
    return msgspec.json.encode(
        {
            "data": data,
            "meta": {
                "api_version": "1.0",
                "request_id": "req_123",
                "timestamp": "2026-03-08T00:00:00Z",
            },
        }
    )


@respx.mock
@pytest.mark.asyncio
async def test_get_devices(client: HypercolorClient) -> None:
    route = respx.get("http://hyperia.test:9420/api/v1/devices").mock(
        return_value=httpx.Response(
            200,
            content=_envelope(
                {
                    "items": [
                        {
                            "id": "keyboard",
                            "layout_device_id": "keyboard",
                            "name": "Keyboard",
                            "backend": "hid",
                            "status": "connected",
                            "brightness": 88,
                            "firmware_version": None,
                            "total_leds": 104,
                            "zones": [
                                {
                                    "id": "main",
                                    "name": "Main",
                                    "led_count": 104,
                                    "topology": "matrix",
                                    "topology_hint": {"type": "matrix", "rows": 6, "cols": 18},
                                }
                            ],
                            "connection_label": "USB HID",
                            "network_ip": None,
                            "network_hostname": None,
                        }
                    ],
                    "pagination": {"offset": 0, "limit": 50, "total": 1, "has_more": False},
                }
            ),
        )
    )

    devices = await client.get_devices()

    assert route.called
    assert len(devices) == 1
    assert devices[0].name == "Keyboard"
    assert devices[0].enabled is True
    assert devices[0].brightness == 88


@respx.mock
@pytest.mark.asyncio
async def test_get_active_effect_returns_none_on_404(client: HypercolorClient) -> None:
    respx.get("http://hyperia.test:9420/api/v1/effects/active").mock(
        return_value=httpx.Response(
            404,
            content=msgspec.json.encode(
                {
                    "error": {
                        "code": "not_found",
                        "message": "No effect is active",
                        "details": {},
                    },
                    "meta": {
                        "api_version": "1.0",
                        "request_id": "req_404",
                        "timestamp": "2026-03-08T00:00:00Z",
                    },
                }
            ),
        )
    )

    assert await client.get_active_effect() is None


@respx.mock
@pytest.mark.asyncio
async def test_get_active_effect_decodes_live_state(client: HypercolorClient) -> None:
    respx.get("http://hyperia.test:9420/api/v1/effects/active").mock(
        return_value=httpx.Response(
            200,
            content=_envelope(
                {
                    "id": "aurora",
                    "name": "Aurora",
                    "state": "running",
                    "controls": [
                        {
                            "id": "speed",
                            "label": "Speed",
                            "type": "number",
                            "min": 0,
                            "max": 100,
                            "step": 1,
                            "default": 40,
                        }
                    ],
                    "control_values": {"speed": 72},
                    "active_preset_id": None,
                }
            ),
        )
    )

    effect = await client.get_active_effect()

    assert isinstance(effect, ActiveEffect)
    assert effect.state == "running"
    assert effect.control_values["speed"] == 72


@respx.mock
@pytest.mark.asyncio
async def test_apply_effect(client: HypercolorClient) -> None:
    route = respx.post("http://hyperia.test:9420/api/v1/effects/aurora/apply").mock(
        return_value=httpx.Response(
            200,
            content=_envelope(
                {
                    "effect": {"id": "aurora", "name": "Aurora"},
                    "applied_controls": {"effectSpeed": 70},
                    "layout": {
                        "associated_layout_id": "desk",
                        "resolved": True,
                        "applied": True,
                    },
                    "transition": {"type": "cut", "duration_ms": 0},
                }
            ),
        )
    )

    result = await client.apply_effect("aurora", controls={"effectSpeed": 70})

    assert route.called
    assert result.effect.name == "Aurora"
    assert result.applied_controls["effectSpeed"] == 70
    assert result.layout is not None
    assert result.layout["associated_layout_id"] == "desk"


@respx.mock
@pytest.mark.asyncio
async def test_get_effect_raises_not_found(client: HypercolorClient) -> None:
    respx.get("http://hyperia.test:9420/api/v1/effects/missing").mock(
        return_value=httpx.Response(
            404,
            content=msgspec.json.encode(
                {
                    "error": {
                        "code": "not_found",
                        "message": "Effect not found",
                        "details": {},
                    },
                    "meta": {
                        "api_version": "1.0",
                        "request_id": "req_missing",
                        "timestamp": "2026-03-08T00:00:00Z",
                    },
                }
            ),
        )
    )

    with pytest.raises(HypercolorNotFoundError):
        await client.get_effect("missing")


@respx.mock
@pytest.mark.asyncio
async def test_update_controls_wraps_controls_payload(client: HypercolorClient) -> None:
    route = respx.patch("http://hyperia.test:9420/api/v1/effects/current/controls").mock(
        return_value=httpx.Response(
            200,
            content=_envelope(
                {
                    "effect": "Aurora",
                    "applied": {"speed": 80},
                    "rejected": [],
                }
            ),
        )
    )

    result = await client.update_controls({"speed": 80})

    assert route.called
    assert json.loads(route.calls[0].request.content) == {"controls": {"speed": 80}}
    assert result.applied["speed"] == 80


@respx.mock
@pytest.mark.asyncio
async def test_update_controls_raises_validation_error(client: HypercolorClient) -> None:
    respx.patch("http://hyperia.test:9420/api/v1/effects/current/controls").mock(
        return_value=httpx.Response(
            422,
            content=msgspec.json.encode(
                {
                    "error": {
                        "code": "validation_error",
                        "message": "Bad control value",
                        "details": {"control": "effectSpeed"},
                    },
                    "meta": {
                        "api_version": "1.0",
                        "request_id": "req_bad",
                        "timestamp": "2026-03-08T00:00:00Z",
                    },
                }
            ),
        )
    )

    with pytest.raises(HypercolorValidationError):
        await client.update_controls({"effectSpeed": 200})


@respx.mock
@pytest.mark.asyncio
async def test_health(client: HypercolorClient) -> None:
    respx.get("http://hyperia.test:9420/health").mock(
        return_value=httpx.Response(
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
    )

    health = await client.health()

    assert health.status == "healthy"
    assert health.checks["render_loop"] == "ok"


@respx.mock
@pytest.mark.asyncio
async def test_get_status_uses_current_daemon_shape(client: HypercolorClient) -> None:
    respx.get("http://hyperia.test:9420/api/v1/status").mock(
        return_value=httpx.Response(
            200,
            content=_envelope(
                {
                    "running": True,
                    "version": "0.1.0",
                    "server": {
                        "instance_id": "srv_1",
                        "instance_name": "Hyperia",
                        "version": "0.1.0",
                    },
                    "config_path": "/var/lib/hypercolor/hypercolor.toml",
                    "data_dir": "/var/lib/hypercolor/data",
                    "cache_dir": "/var/cache/hypercolor",
                    "uptime_seconds": 42,
                    "device_count": 2,
                    "effect_count": 9,
                    "scene_count": 3,
                    "active_effect": "Aurora",
                    "global_brightness": 65,
                    "audio_available": True,
                    "capture_available": False,
                    "render_loop": {
                        "state": "running",
                        "fps_tier": "high",
                        "total_frames": 1024,
                    },
                    "event_bus_subscribers": 4,
                }
            ),
        )
    )

    status = await client.get_status()

    assert status.global_brightness == 65
    assert status.brightness == 65
    assert status.paused is False
    assert status.active_effect == "Aurora"


@pytest.mark.asyncio
async def test_connect_error_is_wrapped() -> None:
    def handler(_: httpx.Request) -> httpx.Response:
        raise httpx.ConnectError("boom")

    transport = httpx.MockTransport(handler)
    async with HypercolorClient(host="hyperia.test", port=9420, transport=transport) as client:
        with pytest.raises(HypercolorConnectionError):
            await client.get_status()


@respx.mock
@pytest.mark.asyncio
async def test_get_effect_decodes_full_model(client: HypercolorClient) -> None:
    respx.get("http://hyperia.test:9420/api/v1/effects/aurora").mock(
        return_value=httpx.Response(
            200,
            content=_envelope(
                {
                    "id": "aurora",
                    "name": "Aurora",
                    "description": "Northern lights",
                    "author": "SignalRGB",
                    "category": "ambient",
                    "source": "native",
                    "runnable": True,
                    "tags": ["nature"],
                    "version": "1.2.3",
                    "audio_reactive": False,
                    "controls": [
                        {
                            "id": "effectSpeed",
                            "label": "Animation Speed",
                            "type": "number",
                            "min": 0,
                            "max": 100,
                            "step": 1,
                            "default": 40,
                        }
                    ],
                    "presets": [{"name": "Default", "is_default": True}],
                    "active_control_values": {"effectSpeed": 40},
                }
            ),
        )
    )

    effect = await client.get_effect("aurora")

    assert isinstance(effect, Effect)
    assert effect.controls[0].label == "Animation Speed"
    assert effect.active_control_values == {"effectSpeed": 40}


@respx.mock
@pytest.mark.asyncio
async def test_get_audio_devices(client: HypercolorClient) -> None:
    respx.get("http://hyperia.test:9420/api/v1/audio/devices").mock(
        return_value=httpx.Response(
            200,
            content=_envelope(
                {
                    "devices": [
                        {
                            "id": "default",
                            "name": "System Monitor (Auto)",
                            "description": "Prefer the active system output monitor source",
                        }
                    ],
                    "current": "default",
                }
            ),
        )
    )

    result = await client.get_audio_devices()

    assert result.current == "default"
    assert result.devices[0].name == "System Monitor (Auto)"


@respx.mock
@pytest.mark.asyncio
async def test_set_audio_device_uses_config_api(client: HypercolorClient) -> None:
    route = respx.post("http://hyperia.test:9420/api/v1/config/set").mock(
        return_value=httpx.Response(
            200,
            content=_envelope(
                {
                    "key": "audio.device",
                    "value": "default",
                    "live": True,
                    "path": "/var/lib/hypercolor/hypercolor.toml",
                }
            ),
        )
    )

    result = await client.set_audio_device("default")

    assert route.called
    assert json.loads(route.calls[0].request.content) == {
        "key": "audio.device",
        "value": '"default"',
        "live": True,
    }
    assert result.key == "audio.device"
