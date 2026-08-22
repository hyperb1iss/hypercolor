"""Tests for the async Hypercolor client."""

from __future__ import annotations

import json
from datetime import timedelta
from pathlib import Path
from typing import cast

import httpx
import msgspec
import pytest
import respx

from hypercolor._generated.types import Unset
from hypercolor.client import HypercolorClient
from hypercolor.exceptions import (
    HypercolorApiError,
    HypercolorAuthenticationError,
    HypercolorConnectionError,
    HypercolorNotFoundError,
)
from hypercolor.models import EffectDetailResponse, EffectPresetOrigin
from hypercolor.models.control import ControlSurface
from hypercolor.models.driver import Driver

_SYSTEM_STATUS_FIXTURE = Path(__file__).with_name("fixtures") / "system_status.json"


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


def _system_status_payload() -> dict[str, object]:
    payload = json.loads(_SYSTEM_STATUS_FIXTURE.read_text())
    assert isinstance(payload, dict)
    return payload


def _error(message: str, code: str = "not_found") -> bytes:
    return msgspec.json.encode(
        {
            "error": {"code": code, "message": message, "details": {}},
            "meta": {
                "api_version": "1.0",
                "request_id": "req_error",
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


def _applied_zone(effect_id: str = "aurora") -> dict[str, object]:
    return {
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
                    "effect_id": effect_id,
                    "controls": {"effectSpeed": {"kind": "int", "value": 70}},
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
    }


def _device_with_attachments() -> dict[str, object]:
    return {
        "id": "controller",
        "layout_device_id": "controller",
        "name": "Controller",
        "origin": {"driver_id": "hid", "backend_id": "hid", "transport": "usb"},
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
                    "suggested_categories": ["strip"],
                    "allowed_templates": ["strip-60"],
                    "allow_custom": True,
                }
            ],
            "bindings": [
                {
                    "slot_id": "channel-1",
                    "template_id": "strip-60",
                    "template_name": "60 LED Strip",
                    "name": None,
                    "enabled": True,
                    "instances": 1,
                    "led_offset": 0,
                    "effective_led_count": 60,
                }
            ],
            "suggested_zones": [
                {
                    "slot_id": "channel-1",
                    "template_id": "strip-60",
                    "template_name": "60 LED Strip",
                    "name": "Channel 1",
                    "instance": 0,
                    "led_start": 0,
                    "led_count": 60,
                    "category": "strip",
                    "default_size": {"width": 0.25, "height": 0.25},
                    "topology": {"type": "strip", "count": 60},
                    "led_mapping": None,
                }
            ],
        },
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
                            "origin": {
                                "driver_id": "hid",
                                "backend_id": "hid",
                                "transport": "usb",
                            },
                            "status": "connected",
                            "brightness": 88,
                            "firmware_version": None,
                            "total_leds": 104,
                            "segments": [
                                {
                                    "id": "main",
                                    "name": "Main",
                                    "led_count": 104,
                                    "topology": "matrix",
                                    "topology_hint": {"type": "matrix", "rows": 6, "cols": 18},
                                }
                            ],
                            "connection": {"transport": "usb", "label": "USB HID"},
                        }
                    ],
                    "total": 1,
                    "page": {"offset": 0, "limit": 50, "has_more": False},
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
async def test_get_devices_accepts_origin_connection_shape(
    client: HypercolorClient,
) -> None:
    route = respx.get("http://hyperia.test:9420/api/v1/devices").mock(
        return_value=httpx.Response(
            200,
            content=_envelope(
                {
                    "items": [
                        {
                            "id": "wled-studio",
                            "layout_device_id": "wled:c8c9a33a9091",
                            "name": "WLED - Studio",
                            "origin": {
                                "driver_id": "wled",
                                "backend_id": "wled",
                                "transport": "network",
                            },
                            "presentation": {
                                "label": "WLED",
                                "short_label": "WLED",
                                "icon": "lightbulb",
                            },
                            "status": "known",
                            "brightness": 100,
                            "firmware_version": "0.15.0-b3",
                            "connection": {
                                "transport": "network",
                                "endpoint": "wled-studio.local",
                                "ip": "10.4.22.169",
                                "hostname": "wled-studio.local",
                            },
                            "total_leds": 275,
                            "segments": [
                                {
                                    "id": "zone_0",
                                    "name": "Main",
                                    "led_count": 275,
                                    "topology": "strip",
                                    "topology_hint": {"type": "strip"},
                                }
                            ],
                        }
                    ],
                    "total": 1,
                    "page": {"offset": 0, "limit": 50, "has_more": False},
                }
            ),
        )
    )

    devices = await client.get_devices()

    assert route.called
    assert devices[0].origin is not None
    assert devices[0].origin.backend_id == "wled"
    assert devices[0].connection is not None
    assert devices[0].connection.ip == "10.4.22.169"
    assert devices[0].connection.endpoint == "wled-studio.local"


@respx.mock
@pytest.mark.asyncio
async def test_get_devices_sends_canonical_backend_id_filter(
    client: HypercolorClient,
) -> None:
    route = respx.get("http://hyperia.test:9420/api/v1/devices").mock(
        return_value=httpx.Response(
            200,
            content=_envelope(
                {
                    "items": [],
                    "total": 0,
                    "page": {"offset": 0, "limit": 50, "has_more": False},
                }
            ),
        )
    )

    devices = await client.get_devices(backend_id="hid", driver="razer")

    assert route.called
    params = route.calls[0].request.url.params
    assert params["backend_id"] == "hid"
    assert params["driver"] == "razer"
    assert devices == []


@respx.mock
@pytest.mark.asyncio
async def test_get_devices_preserves_included_attachments(client: HypercolorClient) -> None:
    route = respx.get("http://hyperia.test:9420/api/v1/devices").mock(
        return_value=httpx.Response(
            200,
            content=_envelope(
                {
                    "items": [_device_with_attachments()],
                    "total": 1,
                    "page": {
                        "offset": 0,
                        "limit": 50,
                        "has_more": False,
                    },
                }
            ),
        )
    )

    devices = await client.get_devices(include="attachments")

    assert route.calls[0].request.url.params["include"] == "attachments"
    attachments = devices[0].attachments
    assert attachments is not None
    assert attachments.slots[0].allowed_templates == ["strip-60"]
    assert attachments.bindings[0].effective_led_count == 60
    assert attachments.suggested_zones[0].topology == {"type": "strip", "count": 60}


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
                    "origin": {
                        "driver_id": "hid",
                        "backend_id": "hid",
                        "transport": "usb",
                    },
                    "status": "connected",
                    "brightness": 88,
                    "firmware_version": None,
                    "total_leds": 104,
                    "segments": [
                        {
                            "id": "main",
                            "name": "Main",
                            "led_count": 104,
                            "topology": "matrix",
                            "topology_hint": {"type": "matrix", "rows": 6, "cols": 18},
                        }
                    ],
                    "connection": {"transport": "usb", "label": "USB HID"},
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
async def test_effect_cover_image_url_is_absolute(client: HypercolorClient) -> None:
    assert (
        client.effect_cover_image_url("aurora/main")
        == "http://hyperia.test:9420/api/v1/effects/aurora%2Fmain/cover"
    )


@respx.mock
@pytest.mark.asyncio
async def test_apply_effect(client: HypercolorClient) -> None:
    route = respx.post("http://hyperia.test:9420/api/v1/effects/aurora%2Fmain/apply").mock(
        return_value=httpx.Response(
            200,
            content=_envelope(
                {
                    "zone": _applied_zone("aurora/main"),
                    "transition": {"type": "cut"},
                    "output": {"applied": True},
                }
            ),
        )
    )

    result = await client.apply_effect(
        "aurora/main",
        controls={"effectSpeed": 70},
        transition="cut",
        if_match=7,
    )

    assert route.called
    assert json.loads(route.calls[0].request.content) == {
        "controls": {"effectSpeed": {"kind": "int", "value": 70}},
        "transition": {"type": "cut"},
    }
    assert route.calls[0].request.headers["if-match"] == '"7"'
    assert result.zone.layers[0].source["effect_id"] == "aurora/main"
    assert result.output.applied is True


@respx.mock
@pytest.mark.asyncio
async def test_apply_effect_omits_empty_body(client: HypercolorClient) -> None:
    route = respx.post("http://hyperia.test:9420/api/v1/effects/aurora/apply").mock(
        return_value=httpx.Response(
            200,
            content=_envelope(
                {
                    "zone": _applied_zone(),
                    "transition": {"type": "cut"},
                    "output": {"applied": True},
                }
            ),
        )
    )

    result = await client.apply_effect("aurora")

    assert route.called
    assert route.calls[0].request.content == b""
    assert "content-type" not in route.calls[0].request.headers
    assert result.zone.layers[0].source["effect_id"] == "aurora"


@respx.mock
@pytest.mark.asyncio
async def test_effect_preset_stack_lists_and_applies_both_origins(
    client: HypercolorClient,
) -> None:
    list_route = respx.get("http://hyperia.test:9420/api/v1/effects/aurora%2Fmain/presets").mock(
        return_value=httpx.Response(
            200,
            content=_envelope(
                {
                    "items": [
                        {
                            "id": "bundled-calm",
                            "name": "Calm",
                            "description": None,
                            "effect_id": "aurora/main",
                            "controls": {"speed": {"kind": "float", "value": 0.4}},
                            "tags": [],
                            "origin": "bundled",
                            "editable": False,
                        },
                        {
                            "id": "saved-bright",
                            "name": "Bright",
                            "description": "Custom",
                            "effect_id": "aurora/main",
                            "controls": {"speed": {"kind": "float", "value": 0.8}},
                            "tags": ["custom"],
                            "origin": "saved",
                            "editable": True,
                        },
                    ],
                    "total": 2,
                }
            ),
        )
    )
    apply_route = respx.post(
        "http://hyperia.test:9420/api/v1/effects/aurora%2Fmain/presets/bundled-calm/apply"
    ).mock(
        return_value=httpx.Response(
            200,
            content=_envelope(
                {
                    "zone": _applied_zone("aurora/main"),
                    "transition": {"type": "cut"},
                    "output": {"applied": True},
                }
            ),
        )
    )

    presets = await client.get_effect_presets("aurora/main")
    result = await client.apply_effect_preset(
        "aurora/main",
        "bundled-calm",
        zone="zone-left",
        if_match=9,
    )

    assert list_route.called
    assert presets[0].origin is EffectPresetOrigin.BUNDLED
    assert presets[0].editable is False
    assert presets[1].origin is EffectPresetOrigin.SAVED
    assert presets[1].editable is True
    assert json.loads(apply_route.calls[0].request.content) == {"zone": "zone-left"}
    assert apply_route.calls[0].request.headers["if-match"] == '"9"'
    assert result.zone.layers[0].source["effect_id"] == "aurora/main"


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
async def test_set_brightness_patches_the_output_resource(
    client: HypercolorClient,
) -> None:
    route = respx.patch("http://hyperia.test:9420/api/v1/output").mock(
        return_value=httpx.Response(
            200,
            content=_envelope({"power": "running", "brightness": 0.42}),
        )
    )

    result = await client.set_brightness(0.42)

    assert route.called
    assert json.loads(route.calls[0].request.content) == {"brightness": 0.42}
    assert result.brightness == 0.42
    assert result.brightness_percent == 42
    assert result.paused is False


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
async def test_pause_and_resume_preserve_effect_state(client: HypercolorClient) -> None:
    route = respx.patch("http://hyperia.test:9420/api/v1/output").mock(
        side_effect=[
            httpx.Response(200, content=_envelope({"power": "paused", "brightness": 1.0})),
            httpx.Response(200, content=_envelope({"power": "running", "brightness": 1.0})),
        ]
    )

    paused = await client.pause_rendering()

    assert paused.paused is True
    assert route.calls[0].request.content == b'{"power":"paused"}'

    running = await client.resume_rendering()

    assert running.paused is False
    assert route.calls[1].request.content == b'{"power":"running"}'


@pytest.mark.asyncio
async def test_set_output_refuses_a_patch_that_sets_nothing(
    client: HypercolorClient,
) -> None:
    # The daemon answers 422 for an empty patch; the client refuses to
    # spend the round trip.
    with pytest.raises(ValueError, match="power, brightness, or both"):
        await client.set_output()


@respx.mock
@pytest.mark.asyncio
async def test_activate_scene_quotes_generated_path_parameters(
    client: HypercolorClient,
) -> None:
    route = respx.post("http://hyperia.test:9420/api/v1/scenes/movie%2Fnight/activate").mock(
        return_value=httpx.Response(
            200,
            content=_envelope(
                {
                    "scene": {
                        "id": "movie/night",
                        "name": "Movie Night",
                    },
                    "activated": True,
                    "layout": {"layout_id": "layout-a", "applied": True},
                    "brightness": {"applied": False},
                }
            ),
        )
    )

    result = await client.activate_scene("movie/night", transition_ms=250)

    assert route.called
    assert route.calls[0].request.content == b'{"transition_ms":250}'
    assert result.scene.id == "movie/night"
    assert result.layout.layout_id == "layout-a"
    assert result.brightness.applied is False


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
async def test_patch_layer_controls_addresses_real_layer(client: HypercolorClient) -> None:
    zone = "0193d2c0-0000-7000-8000-000000000001"
    layer = "0193d2c0-0000-7000-8000-000000000002"
    route = respx.patch(
        f"http://hyperia.test:9420/api/v1/scene/zones/{zone}/layers/{layer}/controls"
    ).mock(
        return_value=httpx.Response(
            200,
            content=_envelope(_applied_zone()),
        )
    )

    result = await client.patch_layer_controls(
        zone,
        layer,
        {"speed": 80, "tint": "#8040ff"},
        clear_bindings=["speed"],
    )

    assert route.called
    request = json.loads(route.calls[0].request.content)
    assert request["values"]["speed"] == {"kind": "int", "value": 80}
    assert request["clear_bindings"] == ["speed"]
    assert request["values"]["tint"]["kind"] == "color_linear"
    assert request["values"]["tint"]["value"] == pytest.approx(
        {
            "r": 0.21586050011389926,
            "g": 0.05126945837404324,
            "b": 1.0,
            "a": 1.0,
        }
    )
    assert str(result.layers[0].id) == layer


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
    assert surfaces[0].surface_id == "device:keyboard"


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
                    {"brightness": {"kind": "int", "value": 88}},
                )
            ),
        )
    )

    surface = await client.get_device_controls("keyboard/main")

    assert route.called
    assert surface.surface_id == "device:keyboard/main"
    assert surface.values["brightness"] == {"kind": "int", "value": 88}


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
                    "values": {"brightness": {"kind": "int", "value": 88}},
                }
            ),
        )
    )

    result = await client.set_control_values(
        "device:keyboard",
        {"brightness": 88, "enabled": True},
    )

    assert route.called
    assert json.loads(route.calls[0].request.content) == {
        "values": {
            "brightness": {"kind": "int", "value": 88},
            "enabled": {"kind": "bool", "value": True},
        }
    }
    assert result.revision == 5
    assert result.values["brightness"] == {"kind": "int", "value": 88}


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
                    "result": {"kind": "text", "value": "Identifying keyboard"},
                }
            ),
        )
    )

    result = await client.invoke_control_action(
        "device:keyboard",
        "identify",
        {
            "duration": timedelta(milliseconds=750),
            "attempt": 2,
            "label": "keyboard",
            "options": {"force": True},
            "color": {"kind": "color_rgb", "value": [128, 255, 234]},
            "mac": {"kind": "mac", "value": "aabb.ccdd.eeff"},
            "nested": {
                "kind": "map",
                "value": {
                    "attempt": 3,
                    "flags": {"kind": "list", "value": [True, "safe"]},
                },
            },
        },
    )

    assert route.called
    assert json.loads(route.calls[0].request.content) == {
        "input": {
            "duration": {"kind": "duration", "value": 750},
            "attempt": {"kind": "int", "value": 2},
            "label": {"kind": "text", "value": "keyboard"},
            "options": {
                "kind": "map",
                "value": {"force": {"kind": "bool", "value": True}},
            },
            "color": {
                "kind": "color_rgb",
                "value": {"r": 128, "g": 255, "b": 234},
            },
            "mac": {"kind": "mac", "value": "aabb.ccdd.eeff"},
            "nested": {
                "kind": "map",
                "value": {
                    "attempt": {"kind": "int", "value": 3},
                    "flags": {
                        "kind": "list",
                        "value": [
                            {"kind": "bool", "value": True},
                            {"kind": "text", "value": "safe"},
                        ],
                    },
                },
            },
        }
    }
    assert result.status == "completed"
    assert result.result == {"kind": "text", "value": "Identifying keyboard"}


@pytest.mark.parametrize(
    "malformed",
    [
        {"kind": "bool"},
        {"kind": "null", "value": None},
        {"kind": "text", "value": "ok", "extra": True},
        {"kind": "future", "value": 1},
        {"kind": "color_rgb", "value": {"r": 1, "g": 2}},
        {"kind": "map", "value": {"nested": {"kind": "int"}}},
    ],
)
@pytest.mark.asyncio
async def test_set_control_values_rejects_malformed_canonical_envelopes(
    client: HypercolorClient,
    malformed: object,
) -> None:
    with pytest.raises((TypeError, ValueError)):
        await client.set_control_values("device:keyboard", {"bad": malformed})


@respx.mock
@pytest.mark.asyncio
async def test_plain_maps_with_business_kind_fields_remain_maps(
    client: HypercolorClient,
) -> None:
    route = respx.patch(
        "http://hyperia.test:9420/api/v1/control-surfaces/device%3Akeyboard/values"
    ).mock(
        return_value=httpx.Response(
            200,
            content=_envelope(
                {
                    "surface_id": "device:keyboard",
                    "previous_revision": 3,
                    "revision": 4,
                    "accepted": [],
                    "rejected": [],
                    "impacts": [],
                    "values": {},
                }
            ),
        )
    )

    await client.set_control_values(
        "device:keyboard",
        {"transport": {"kind": "network", "name": "fixture"}},
    )

    assert json.loads(route.calls[0].request.content)["values"]["transport"] == {
        "kind": "map",
        "value": {
            "kind": {"kind": "text", "value": "network"},
            "name": {"kind": "text", "value": "fixture"},
        },
    }


@respx.mock
@pytest.mark.asyncio
async def test_bare_lists_remain_lists_in_action_input(client: HypercolorClient) -> None:
    route = respx.post(
        "http://hyperia.test:9420/api/v1/control-surfaces/device%3Akeyboard/actions/test"
    ).mock(
        return_value=httpx.Response(
            200,
            content=_envelope(
                {
                    "surface_id": "device:keyboard",
                    "action_id": "test",
                    "status": "completed",
                    "revision": 5,
                }
            ),
        )
    )

    await client.invoke_control_action(
        "device:keyboard",
        "test",
        {"channels": [0.1, 0.2, 0.3, 1.0]},
    )

    assert json.loads(route.calls[0].request.content)["input"]["channels"] == {
        "kind": "list",
        "value": [
            {"kind": "float", "value": 0.1},
            {"kind": "float", "value": 0.2},
            {"kind": "float", "value": 0.3},
            {"kind": "float", "value": 1.0},
        ],
    }


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
                    "checks": {
                        "render_loop": "ok",
                        "device_backends": "ok",
                        "event_bus": "ok",
                    },
                }
            ),
        )
    )

    health = await client.health()

    assert health.status == "healthy"
    assert health.checks.render_loop == "ok"


@respx.mock
@pytest.mark.asyncio
async def test_health_rejects_an_incomplete_projection(client: HypercolorClient) -> None:
    respx.get("http://hyperia.test:9420/health").mock(
        return_value=httpx.Response(200, json={"status": "healthy"})
    )

    with pytest.raises(HypercolorApiError, match="Malformed Hypercolor health response"):
        await client.health()


@respx.mock
@pytest.mark.asyncio
async def test_health_rejects_wrong_primitive_types(client: HypercolorClient) -> None:
    respx.get("http://hyperia.test:9420/health").mock(
        return_value=httpx.Response(
            200,
            json={
                "status": 7,
                "version": [],
                "uptime_seconds": "42",
                "checks": {
                    "render_loop": 1,
                    "device_backends": 2,
                    "event_bus": 3,
                },
            },
        )
    )

    with pytest.raises(HypercolorApiError, match="Malformed Hypercolor health response"):
        await client.health()


@respx.mock
@pytest.mark.asyncio
async def test_get_status_uses_current_daemon_shape(client: HypercolorClient) -> None:
    status_payload = _system_status_payload()
    status_payload.update(
        {
            "running": True,
            "version": "0.1.0",
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
            "event_bus_subscribers": 4,
        }
    )
    render_loop_value = status_payload["render_loop"]
    assert isinstance(render_loop_value, dict)
    render_loop = cast("dict[str, object]", render_loop_value)
    render_loop["state"] = "running"
    render_loop["fps_tier"] = "high"
    render_loop["total_frames"] = 1024
    respx.get("http://hyperia.test:9420/api/v1/system").mock(
        return_value=httpx.Response(
            200,
            content=_envelope(
                {
                    "identity": {
                        "instance_id": "srv_1",
                        "instance_name": "Hyperia",
                        "version": "0.1.0",
                        "device_count": 2,
                        "auth_required": True,
                    },
                    "status": status_payload,
                }
            ),
        )
    )

    status = await client.get_status()

    assert status.global_brightness == 65
    assert status.render_loop.state == "running"
    assert status.active_effect == "Aurora"


@respx.mock
@pytest.mark.asyncio
async def test_get_status_rejects_an_incomplete_projection(client: HypercolorClient) -> None:
    respx.get("http://hyperia.test:9420/api/v1/system").mock(
        return_value=httpx.Response(
            200,
            content=_envelope(
                {
                    "identity": {
                        "instance_id": "srv_1",
                        "instance_name": "Hyperia",
                        "version": "0.1.0",
                        "device_count": 2,
                        "auth_required": True,
                    },
                    "status": {"running": True},
                }
            ),
        )
    )

    with pytest.raises(HypercolorApiError, match="Malformed Hypercolor system status"):
        await client.get_status()


@respx.mock
@pytest.mark.asyncio
async def test_get_status_rejects_wrong_primitive_types(client: HypercolorClient) -> None:
    status_payload = _system_status_payload()
    status_payload["running"] = "yes"
    respx.get("http://hyperia.test:9420/api/v1/system").mock(
        return_value=httpx.Response(
            200,
            content=_envelope(
                {
                    "identity": {
                        "instance_id": "srv_1",
                        "instance_name": "Hyperia",
                        "version": "0.1.0",
                        "device_count": 2,
                        "auth_required": True,
                    },
                    "status": status_payload,
                }
            ),
        )
    )

    with pytest.raises(HypercolorApiError, match="Malformed Hypercolor system status"):
        await client.get_status()


@respx.mock
@pytest.mark.asyncio
async def test_get_status_rejects_wrong_nested_primitive_types(
    client: HypercolorClient,
) -> None:
    status_payload = _system_status_payload()
    render_loop_value = status_payload["render_loop"]
    assert isinstance(render_loop_value, dict)
    render_loop = cast("dict[str, object]", render_loop_value)
    render_loop["target_fps"] = "60"
    respx.get("http://hyperia.test:9420/api/v1/system").mock(
        return_value=httpx.Response(
            200,
            content=_envelope(
                {
                    "identity": {
                        "instance_id": "srv_1",
                        "instance_name": "Hyperia",
                        "version": "0.1.0",
                        "device_count": 2,
                        "auth_required": True,
                    },
                    "status": status_payload,
                }
            ),
        )
    )

    with pytest.raises(HypercolorApiError, match="Malformed Hypercolor system status"):
        await client.get_status()


@respx.mock
@pytest.mark.asyncio
async def test_get_status_requires_authenticated_system_projection(
    client: HypercolorClient,
) -> None:
    respx.get("http://hyperia.test:9420/api/v1/system").mock(
        return_value=httpx.Response(
            200,
            content=_envelope(
                {
                    "identity": {
                        "instance_id": "srv_1",
                        "instance_name": "Hyperia",
                        "version": "0.1.0",
                        "device_count": 2,
                        "auth_required": True,
                    }
                }
            ),
        )
    )

    with pytest.raises(HypercolorAuthenticationError):
        await client.get_status()


@respx.mock
@pytest.mark.asyncio
async def test_response_envelope_requires_metadata(client: HypercolorClient) -> None:
    respx.get("http://hyperia.test:9420/api/v1/output").mock(
        return_value=httpx.Response(
            200,
            json={"data": {"power": "running", "brightness": 1.0}},
        )
    )

    with pytest.raises(HypercolorApiError, match="Unexpected Hypercolor response envelope"):
        await client.get_output()


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
                            "tags": [],
                            "version": "1.0.0",
                        }
                    ],
                    "total": 1,
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
                    "total": 1,
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
    assert deleted == {"id": "preset-b", "deleted": True}


@respx.mock
@pytest.mark.asyncio
async def test_scene_display_and_diagnostics_helpers(
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
    snapshot_route = respx.post("http://hyperia.test:9420/api/v1/scenes/snapshot").mock(
        return_value=httpx.Response(
            201,
            content=_envelope(
                {
                    "id": "scene-snapshot",
                    "name": "Evening",
                    "description": "soft",
                    "enabled": True,
                    "priority": 50,
                    "mutation_mode": "snapshot",
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
                    "zone": {"id": "zone-a"},
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
    snapshot = await client.snapshot_scene(
        "Evening",
        description="soft",
    )
    displays = await client.list_displays()
    face = await client.set_display_face(
        "streamdeck",
        "clock",
        controls={"speed": 0.8},
        opacity=0.8,
    )
    diagnostics = await client.run_diagnostics(checks=["daemon"], system=True)

    assert scene.id == "scene-a"
    assert json.loads(scene_route.calls[0].request.content) == {
        "name": "Desk Glow",
        "enabled": True,
        "mutation_mode": "live",
    }
    assert snapshot.name == "Evening"
    assert snapshot.mutation_mode == "snapshot"
    assert json.loads(snapshot_route.calls[0].request.content) == {
        "name": "Evening",
        "description": "soft",
    }
    assert displays[0].id == "streamdeck"
    assert json.loads(face_route.calls[0].request.content) == {
        "effect_id": "clock",
        "controls": {"speed": {"kind": "float", "value": 0.8}},
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
                            "default_value": {"kind": "int", "value": 40},
                        }
                    ],
                    "presets": [
                        {
                            "id": "default",
                            "name": "Default",
                            "controls": {"effectSpeed": {"kind": "int", "value": 40}},
                        }
                    ],
                }
            ),
        )
    )

    effect = await client.get_effect("aurora")

    assert isinstance(effect, EffectDetailResponse)
    assert not isinstance(effect.controls, Unset)
    assert effect.controls[0].name == "Animation Speed"
    assert not isinstance(effect.presets, Unset)
    assert effect.presets[0].name == "Default"
    assert not isinstance(effect.presets[0].controls, Unset)
    assert effect.presets[0].controls.to_dict() == {"effectSpeed": {"kind": "int", "value": 40}}


@respx.mock
@pytest.mark.asyncio
async def test_get_audio_devices(client: HypercolorClient) -> None:
    respx.get("http://hyperia.test:9420/api/v1/system/audio-devices").mock(
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
    route = respx.put("http://hyperia.test:9420/api/v1/config/keys/audio.device").mock(
        return_value=httpx.Response(
            200,
            content=_envelope(
                {
                    "key": "audio.device",
                    "value": "default",
                    "live": True,
                    "requires_restart": False,
                    "pending_restart": [],
                    "path": "/var/lib/hypercolor/hypercolor.toml",
                }
            ),
        )
    )

    result = await client.set_audio_device("default")

    assert route.called
    request = route.calls[0].request
    assert json.loads(request.content) == "default"
    assert request.url.params["live"] == "true"
    assert result.key == "audio.device"
    assert result.requires_restart is False
    assert result.pending_restart == []
