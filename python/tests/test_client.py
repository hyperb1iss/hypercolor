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
from hypercolor.models.control import ControlSurface
from hypercolor.models.driver import Driver
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


def _control_surface(
    surface_id: str, values: dict[str, object] | None = None
) -> dict[str, object]:
    return {
        "surface_id": surface_id,
        "scope": {"kind": "device", "device_id": "keyboard", "driver_id": "test"},
        "schema_version": 1,
        "revision": 4,
        "groups": [],
        "fields": [],
        "actions": [],
        "values": values or {},
        "availability": {},
        "action_availability": {},
    }


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
async def test_get_devices_maps_backend_alias_to_backend_id(
    client: HypercolorClient,
) -> None:
    route = respx.get("http://hyperia.test:9420/api/v1/devices").mock(
        return_value=httpx.Response(
            200,
            content=_envelope(
                {
                    "items": [],
                    "pagination": {"offset": 0, "limit": 50, "total": 0, "has_more": False},
                }
            ),
        )
    )

    devices = await client.get_devices(backend="hid", driver="razer")

    assert route.called
    params = route.calls[0].request.url.params
    assert params["backend_id"] == "hid"
    assert params["driver"] == "razer"
    assert "backend" not in params
    assert devices == []


@respx.mock
@pytest.mark.asyncio
async def test_get_device_quotes_generated_path_parameters(client: HypercolorClient) -> None:
    route = respx.get("http://hyperia.test:9420/api/v1/devices/keyboard%2Fmain").mock(
        return_value=httpx.Response(
            200,
            content=_envelope(
                {
                    "id": "keyboard/main",
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
            ),
        )
    )

    device = await client.get_device("keyboard/main")

    assert route.called
    assert device.id == "keyboard/main"


@respx.mock
@pytest.mark.asyncio
async def test_get_drivers_decodes_protocol_catalog(client: HypercolorClient) -> None:
    route = respx.get("http://hyperia.test:9420/api/v1/drivers").mock(
        return_value=httpx.Response(
            200,
            content=_envelope(
                {
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
                            "presentation": {"label": "Nollie", "icon": "grid"},
                            "enabled": True,
                            "config_key": "drivers.nollie",
                            "protocols": [
                                {
                                    "driver_id": "nollie",
                                    "protocol_id": "nollie_8",
                                    "display_name": "Nollie 8",
                                    "vendor_id": 0x2E8A,
                                    "product_id": 0x0008,
                                    "family_id": "nollie",
                                    "transport": "usb",
                                    "route_backend_id": "usb",
                                }
                            ],
                        }
                    ]
                }
            ),
        )
    )

    drivers = await client.get_drivers()

    assert route.called
    assert isinstance(drivers[0], Driver)
    assert drivers[0].presentation is not None
    assert drivers[0].presentation.label == "Nollie"
    assert drivers[0].protocols[0].protocol_id == "nollie_8"


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
                            "name": "Speed",
                            "control_type": "slider",
                            "min": 0,
                            "max": 100,
                            "step": 1,
                            "default_value": {"integer": 40},
                        }
                    ],
                    "control_values": {"speed": {"integer": 72}},
                    "active_preset_id": None,
                }
            ),
        )
    )

    effect = await client.get_active_effect()

    assert isinstance(effect, ActiveEffect)
    assert effect.state == "running"
    assert effect.control_values["speed"] == 72
    assert effect.controls[0].label == "Speed"
    assert effect.controls[0].type == "number"
    assert effect.controls[0].default == 40


@respx.mock
@pytest.mark.asyncio
async def test_apply_effect(client: HypercolorClient) -> None:
    route = respx.post("http://hyperia.test:9420/api/v1/effects/aurora%2Fmain/apply").mock(
        return_value=httpx.Response(
            200,
            content=_envelope(
                {
                    "effect": {"id": "aurora/main", "name": "Aurora"},
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

    result = await client.apply_effect("aurora/main", controls={"effectSpeed": 70})

    assert route.called
    assert json.loads(route.calls[0].request.content) == {"controls": {"effectSpeed": 70}}
    assert result.effect.name == "Aurora"
    assert result.applied_controls["effectSpeed"] == 70
    assert result.layout is not None
    assert result.layout["associated_layout_id"] == "desk"


@respx.mock
@pytest.mark.asyncio
async def test_apply_effect_omits_empty_body(client: HypercolorClient) -> None:
    route = respx.post("http://hyperia.test:9420/api/v1/effects/aurora/apply").mock(
        return_value=httpx.Response(
            200,
            content=_envelope(
                {
                    "effect": {"id": "aurora", "name": "Aurora"},
                    "applied_controls": {},
                    "layout": {"resolved": False, "applied": False},
                    "transition": {"type": "cut", "duration_ms": 0},
                }
            ),
        )
    )

    result = await client.apply_effect("aurora")

    assert route.called
    assert route.calls[0].request.content == b""
    assert "content-type" not in route.calls[0].request.headers
    assert result.effect.id == "aurora"


@respx.mock
@pytest.mark.asyncio
async def test_upload_effect_uses_install_endpoint(client: HypercolorClient) -> None:
    route = respx.post("http://hyperia.test:9420/api/v1/effects/install").mock(
        return_value=httpx.Response(
            201,
            content=_envelope(
                {
                    "id": "user:neon",
                    "name": "Neon",
                    "source": "user",
                    "path": "/effects/neon.html",
                    "controls": 2,
                    "presets": 1,
                }
            ),
        )
    )

    result = await client.upload_effect("neon.html", "<html></html>")

    assert route.called
    request = route.calls[0].request
    assert "multipart/form-data" in request.headers["content-type"]
    assert b'name="file"; filename="neon.html"' in request.content
    assert result["id"] == "user:neon"


@respx.mock
@pytest.mark.asyncio
async def test_set_brightness_uses_generated_route_with_body(
    client: HypercolorClient,
) -> None:
    route = respx.put("http://hyperia.test:9420/api/v1/settings/brightness").mock(
        return_value=httpx.Response(
            200,
            content=_envelope({"brightness": 42}),
        )
    )

    result = await client.set_brightness(42)

    assert route.called
    assert json.loads(route.calls[0].request.content) == {"brightness": 42}
    assert result.brightness == 42


@respx.mock
@pytest.mark.asyncio
async def test_identify_device_quotes_generated_path_parameters(
    client: HypercolorClient,
) -> None:
    route = respx.post("http://hyperia.test:9420/api/v1/devices/desk%2Flight/identify").mock(
        return_value=httpx.Response(
            200,
            content=_envelope(
                {
                    "device_id": "desk/light",
                    "identifying": True,
                    "duration_ms": 750,
                }
            ),
        )
    )

    result = await client.identify_device("desk/light", duration_ms=750, color="#80ffea")

    assert route.called
    assert json.loads(route.calls[0].request.content) == {
        "duration_ms": 750,
        "color": "#80ffea",
    }
    assert result.device_id == "desk/light"


@respx.mock
@pytest.mark.asyncio
async def test_discover_devices_omits_empty_body(client: HypercolorClient) -> None:
    route = respx.post("http://hyperia.test:9420/api/v1/devices/discover").mock(
        return_value=httpx.Response(
            200,
            content=_envelope({"scan_id": "scan_1", "status": "running"}),
        )
    )

    result = await client.discover_devices()

    assert route.called
    assert route.calls[0].request.content == b""
    assert result.scan_id == "scan_1"


@respx.mock
@pytest.mark.asyncio
async def test_stop_effect_uses_generated_route(client: HypercolorClient) -> None:
    route = respx.post("http://hyperia.test:9420/api/v1/effects/stop").mock(
        return_value=httpx.Response(
            200,
            content=_envelope({"stopped": True}),
        )
    )

    result = await client.stop_effect()

    assert route.called
    assert result.stopped is True


@respx.mock
@pytest.mark.asyncio
async def test_apply_profile_quotes_generated_path_parameters(
    client: HypercolorClient,
) -> None:
    route = respx.post("http://hyperia.test:9420/api/v1/profiles/movie%2Fnight/apply").mock(
        return_value=httpx.Response(
            200,
            content=_envelope(
                {
                    "profile": {
                        "id": "movie/night",
                        "name": "Movie Night",
                    },
                    "applied": True,
                    "transition": {"type": "fade", "duration_ms": 500},
                }
            ),
        )
    )

    result = await client.apply_profile(
        "movie/night",
        transition={"type": "fade", "duration_ms": 500},
    )

    assert route.called
    assert json.loads(route.calls[0].request.content) == {
        "transition": {"type": "fade", "duration_ms": 500}
    }
    assert result.profile.id == "movie/night"


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
async def test_get_control_surfaces_uses_pythonic_filters(client: HypercolorClient) -> None:
    route = respx.get("http://hyperia.test:9420/api/v1/control-surfaces").mock(
        return_value=httpx.Response(
            200,
            content=_envelope(
                {
                    "surfaces": [
                        _control_surface("device:keyboard"),
                    ]
                }
            ),
        )
    )

    surfaces = await client.get_control_surfaces(
        device_id="keyboard/main",
        include_driver=True,
    )

    assert route.called
    assert route.calls[0].request.url.params["device_id"] == "keyboard/main"
    assert route.calls[0].request.url.params["include_driver"] == "true"
    assert isinstance(surfaces[0], ControlSurface)
    assert surfaces[0].id == "device:keyboard"


@respx.mock
@pytest.mark.asyncio
async def test_get_device_controls_quotes_generated_path_parameters(
    client: HypercolorClient,
) -> None:
    route = respx.get("http://hyperia.test:9420/api/v1/devices/keyboard%2Fmain/controls").mock(
        return_value=httpx.Response(
            200,
            content=_envelope(
                _control_surface(
                    "device:keyboard/main",
                    {"brightness": {"kind": "integer", "value": 88}},
                )
            ),
        )
    )

    surface = await client.get_device_controls("keyboard/main")

    assert route.called
    assert surface.id == "device:keyboard/main"
    assert surface.values["brightness"] == 88


@respx.mock
@pytest.mark.asyncio
async def test_set_control_values_converts_python_values(client: HypercolorClient) -> None:
    route = respx.patch(
        "http://hyperia.test:9420/api/v1/control-surfaces/device%3Akeyboard/values"
    ).mock(
        return_value=httpx.Response(
            200,
            content=_envelope(
                {
                    "surface_id": "device:keyboard",
                    "previous_revision": 4,
                    "revision": 5,
                    "accepted": [],
                    "rejected": [],
                    "impacts": [],
                    "values": {"brightness": {"kind": "integer", "value": 88}},
                }
            ),
        )
    )

    result = await client.set_control_values(
        "device:keyboard",
        {"brightness": 88, "enabled": True},
        expected_revision=4,
    )

    assert route.called
    assert json.loads(route.calls[0].request.content) == {
        "surface_id": "device:keyboard",
        "changes": [
            {"field_id": "brightness", "value": {"kind": "integer", "value": 88}},
            {"field_id": "enabled", "value": {"kind": "bool", "value": True}},
        ],
        "expected_revision": 4,
    }
    assert result.revision == 5
    assert result.values["brightness"] == 88


@respx.mock
@pytest.mark.asyncio
async def test_invoke_control_action_converts_input(client: HypercolorClient) -> None:
    route = respx.post(
        "http://hyperia.test:9420/api/v1/control-surfaces/device%3Akeyboard/actions/identify"
    ).mock(
        return_value=httpx.Response(
            200,
            content=_envelope(
                {
                    "surface_id": "device:keyboard",
                    "action_id": "identify",
                    "status": "completed",
                    "revision": 5,
                    "result": {"kind": "string", "value": "Identifying keyboard"},
                }
            ),
        )
    )

    result = await client.invoke_control_action(
        "device:keyboard",
        "identify",
        {"duration_ms": 750, "color": {"kind": "color_rgb", "value": [128, 255, 234]}},
    )

    assert route.called
    assert json.loads(route.calls[0].request.content) == {
        "input": {
            "duration_ms": {"kind": "integer", "value": 750},
            "color": {"kind": "color_rgb", "value": [128, 255, 234]},
        }
    }
    assert result.status == "completed"
    assert result.result == "Identifying keyboard"


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


@pytest.mark.asyncio
async def test_injected_httpx_client_uses_absolute_url_and_request_auth() -> None:
    requests: list[httpx.Request] = []

    def handler(request: httpx.Request) -> httpx.Response:
        requests.append(request)
        return httpx.Response(
            200,
            content=_envelope(
                {
                    "items": [
                        {
                            "id": "aurora",
                            "name": "Aurora",
                            "description": "Northern lights",
                            "author": "Hypercolor",
                            "category": "ambient",
                            "source": "native",
                            "runnable": True,
                            "version": "1.0.0",
                        }
                    ],
                    "pagination": {"offset": 0, "limit": 50, "total": 1, "has_more": False},
                }
            ),
        )

    shared_client = httpx.AsyncClient(transport=httpx.MockTransport(handler))
    client = HypercolorClient(
        host="hyperia.test",
        port=9420,
        api_key="secret",
        httpx_client=shared_client,
    )

    effects = await client.get_effects()
    await client.aclose()

    assert effects[0].id == "aurora"
    assert str(requests[0].url) == "http://hyperia.test:9420/api/v1/effects"
    assert requests[0].headers["authorization"] == "Bearer secret"
    assert shared_client.is_closed is False

    await shared_client.aclose()


@respx.mock
@pytest.mark.asyncio
async def test_library_helpers(client: HypercolorClient) -> None:
    respx.get("http://hyperia.test:9420/api/v1/library/presets").mock(
        return_value=httpx.Response(
            200,
            content=_envelope(
                {
                    "items": [
                        {
                            "id": "preset-a",
                            "name": "Aurora Soft",
                            "description": None,
                            "effect_id": "aurora",
                            "controls": {"speed": 32},
                            "tags": ["soft"],
                            "created_at_ms": 1,
                            "updated_at_ms": 2,
                        }
                    ],
                    "pagination": {"offset": 0, "limit": 50, "total": 1, "has_more": False},
                }
            ),
        )
    )
    create_route = respx.post("http://hyperia.test:9420/api/v1/library/presets").mock(
        return_value=httpx.Response(
            201,
            content=_envelope(
                {
                    "id": "preset-b",
                    "name": "Aurora Bright",
                    "description": "glow",
                    "effect_id": "aurora",
                    "controls": {"speed": 64},
                    "tags": ["bright"],
                    "created_at_ms": 3,
                    "updated_at_ms": 3,
                }
            ),
        )
    )
    respx.post("http://hyperia.test:9420/api/v1/library/presets/preset-b/apply").mock(
        return_value=httpx.Response(
            200,
            content=_envelope(
                {
                    "preset": {"id": "preset-b", "name": "Aurora Bright"},
                    "effect": {"id": "aurora", "name": "Aurora"},
                    "applied_controls": {"speed": 64},
                    "rejected_controls": [],
                    "warnings": [],
                }
            ),
        )
    )
    respx.delete("http://hyperia.test:9420/api/v1/library/presets/preset-b").mock(
        return_value=httpx.Response(200, content=_envelope({"id": "preset-b", "deleted": True}))
    )

    presets = await client.get_presets()
    created = await client.save_preset(
        "Aurora Bright",
        "aurora",
        description="glow",
        controls={"speed": 64},
        tags=["bright"],
    )
    applied = await client.apply_preset("preset-b")
    deleted = await client.delete_preset("preset-b")

    assert presets[0].name == "Aurora Soft"
    assert json.loads(create_route.calls[0].request.content) == {
        "name": "Aurora Bright",
        "description": "glow",
        "effect": "aurora",
        "controls": {"speed": 64},
        "tags": ["bright"],
    }
    assert created.id == "preset-b"
    assert applied.effect.id == "aurora"
    assert deleted == {"id": "preset-b", "deleted": True}


@respx.mock
@pytest.mark.asyncio
async def test_scene_profile_display_and_diagnostics_helpers(
    client: HypercolorClient,
) -> None:
    scene_route = respx.post("http://hyperia.test:9420/api/v1/scenes").mock(
        return_value=httpx.Response(
            201,
            content=_envelope(
                {
                    "id": "scene-a",
                    "name": "Desk Glow",
                    "description": None,
                    "enabled": True,
                    "priority": 10,
                    "mutation_mode": "live",
                }
            ),
        )
    )
    profile_route = respx.post("http://hyperia.test:9420/api/v1/profiles").mock(
        return_value=httpx.Response(
            201,
            content=_envelope(
                {
                    "id": "profile-a",
                    "name": "Evening",
                    "description": "soft",
                    "brightness": 64,
                }
            ),
        )
    )
    respx.get("http://hyperia.test:9420/api/v1/displays").mock(
        return_value=httpx.Response(
            200,
            content=_envelope(
                [
                    {
                        "id": "streamdeck",
                        "name": "Stream Deck",
                        "vendor": "elgato",
                        "family": "stream_deck",
                        "width": 72,
                        "height": 72,
                        "circular": False,
                    }
                ]
            ),
        )
    )
    face_route = respx.put("http://hyperia.test:9420/api/v1/displays/streamdeck/face").mock(
        return_value=httpx.Response(
            200,
            content=_envelope(
                {
                    "device_id": "streamdeck",
                    "scene_id": "scene-a",
                    "effect": {"id": "clock", "name": "Clock"},
                    "group": {"id": "group-a"},
                }
            ),
        )
    )
    diagnostics_route = respx.post("http://hyperia.test:9420/api/v1/diagnose").mock(
        return_value=httpx.Response(
            200,
            content=_envelope(
                {
                    "checks": [
                        {
                            "category": "system",
                            "name": "daemon_running",
                            "status": "pass",
                            "detail": "0.1.0",
                        }
                    ],
                    "summary": {"passed": 1, "warnings": 0, "failed": 0},
                }
            ),
        )
    )

    scene = await client.create_scene("Desk Glow", enabled=True, mutation_mode="live")
    profile = await client.save_profile(
        "Evening",
        description="soft",
        brightness=64,
        force=True,
    )
    displays = await client.list_displays()
    face = await client.set_display_face(
        "streamdeck",
        "clock",
        controls={"accent": "#80ffea"},
        opacity=0.8,
    )
    diagnostics = await client.run_diagnostics(checks=["daemon"], system=True)

    assert scene.id == "scene-a"
    assert json.loads(scene_route.calls[0].request.content) == {
        "name": "Desk Glow",
        "enabled": True,
        "mutation_mode": "live",
    }
    assert profile.name == "Evening"
    assert json.loads(profile_route.calls[0].request.content) == {
        "name": "Evening",
        "description": "soft",
        "brightness": 64,
        "force": True,
    }
    assert displays[0].id == "streamdeck"
    assert json.loads(face_route.calls[0].request.content) == {
        "effect_id": "clock",
        "controls": {"accent": "#80ffea"},
        "opacity": 0.8,
    }
    assert face.effect["id"] == "clock"
    assert json.loads(diagnostics_route.calls[0].request.content) == {
        "checks": ["daemon"],
        "system": True,
    }
    assert diagnostics["summary"]["passed"] == 1


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
                            "name": "Animation Speed",
                            "control_type": "slider",
                            "min": 0,
                            "max": 100,
                            "step": 1,
                            "default_value": {"integer": 40},
                        }
                    ],
                    "presets": [{"name": "Default", "controls": {"effectSpeed": {"integer": 40}}}],
                    "active_control_values": {"effectSpeed": {"integer": 40}},
                }
            ),
        )
    )

    effect = await client.get_effect("aurora")

    assert isinstance(effect, Effect)
    assert effect.controls[0].label == "Animation Speed"
    assert effect.active_control_values == {"effectSpeed": 40}
    assert effect.presets[0].name == "Default"


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
