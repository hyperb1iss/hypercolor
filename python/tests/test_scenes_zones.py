"""Tests for the scene and zone (render group) client surface."""

from __future__ import annotations

import json
from typing import Any

import httpx
import msgspec
import pytest
import respx

from hypercolor.client import HypercolorClient
from hypercolor.exceptions import HypercolorPreconditionError

SCENE_ID = "0193d2c0-0000-7000-8000-00000000aaaa"
ZONE_ID = "0193d2c0-0000-7000-8000-000000000001"

ZONE_LAYOUT: dict[str, Any] = {
    "id": f"zone-{ZONE_ID}",
    "name": "Desk",
    "description": None,
    "canvas_width": 320,
    "canvas_height": 200,
    "zones": [
        {
            "id": "out-strimer",
            "name": "ATX Strimer",
            "device_id": "usb:controller-1",
            "zone_name": "atx",
            "position": {"x": 0.5, "y": 0.5},
            "size": {"x": 0.4, "y": 0.2},
            "rotation": 0.0,
            "scale": 1.0,
            "display_order": 0,
            "orientation": "horizontal",
            "topology": {"type": "strip", "count": 24, "direction": "left_to_right"},
            "sampling_mode": None,
            "edge_behavior": None,
            "shape": None,
            "shape_preset": None,
        }
    ],
    "default_sampling_mode": {"type": "bilinear"},
    "default_edge_behavior": "clamp",
    "spaces": None,
    "version": 1,
}

ZONE_PAYLOAD: dict[str, Any] = {
    "id": ZONE_ID,
    "name": "Desk",
    "description": None,
    "effect_id": "aurora",
    "controls": {"speed": 50},
    "preset_id": None,
    "layers": [
        {
            "id": ZONE_ID,
            "source": {
                "type": "effect",
                "effect_id": "aurora",
                "controls": {"speed": 50},
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
    "layout": ZONE_LAYOUT,
    "brightness": 0.8,
    "enabled": True,
    "color": "#e135ff",
    "role": "primary",
    "controls_version": 3,
    "layers_version": 1,
}


def _envelope(data: object) -> bytes:
    return msgspec.json.encode(
        {
            "data": data,
            "meta": {
                "api_version": "1.0",
                "request_id": "req_zones",
                "timestamp": "2026-06-11T00:00:00Z",
            },
        }
    )


@respx.mock
@pytest.mark.asyncio
async def test_get_scenes_decodes_mutation_mode(client: HypercolorClient) -> None:
    respx.get("http://hyperia.test:9420/api/v1/scenes").mock(
        return_value=httpx.Response(
            200,
            content=_envelope(
                {
                    "items": [
                        {
                            "id": SCENE_ID,
                            "name": "Battlestation",
                            "description": "Desk rig",
                            "enabled": True,
                            "priority": 50,
                            "mutation_mode": "snapshot",
                        }
                    ],
                    "pagination": {"offset": 0, "limit": 50, "total": 1, "has_more": False},
                }
            ),
        )
    )

    scenes = await client.get_scenes()

    assert scenes[0].mutation_mode == "snapshot"
    assert scenes[0].snapshot_locked is True


@respx.mock
@pytest.mark.asyncio
async def test_get_active_scene_decodes_zones(client: HypercolorClient) -> None:
    respx.get("http://hyperia.test:9420/api/v1/scenes/active").mock(
        return_value=httpx.Response(
            200,
            content=_envelope(
                {
                    "id": SCENE_ID,
                    "name": "Battlestation",
                    "description": None,
                    "enabled": True,
                    "priority": 50,
                    "kind": "named",
                    "mutation_mode": "live",
                    "groups": [ZONE_PAYLOAD],
                    "groups_revision": 12,
                    "unassigned_behavior": "off",
                }
            ),
        )
    )

    scene = await client.get_active_scene()

    assert scene is not None

    assert scene.groups_revision == 12
    assert len(scene.groups) == 1
    zone = scene.groups[0]
    assert zone.is_primary
    assert zone.effect_id == "aurora"
    assert zone.brightness == 0.8
    assert zone.layout.zones[0].led_count == 24
    assert scene.primary_zone is zone
    assert scene.zone(ZONE_ID) is zone


@respx.mock
@pytest.mark.asyncio
async def test_get_active_scene_decodes_fallback_behavior(client: HypercolorClient) -> None:
    respx.get("http://hyperia.test:9420/api/v1/scenes/active").mock(
        return_value=httpx.Response(
            200,
            content=_envelope(
                {
                    "id": SCENE_ID,
                    "name": "Battlestation",
                    "groups": [],
                    "groups_revision": 0,
                    "unassigned_behavior": {"fallback": ZONE_ID},
                }
            ),
        )
    )

    scene = await client.get_active_scene()

    assert scene is not None

    assert scene.unassigned_behavior == {"fallback": ZONE_ID}


@respx.mock
@pytest.mark.asyncio
async def test_create_scene_sends_body(client: HypercolorClient) -> None:
    route = respx.post("http://hyperia.test:9420/api/v1/scenes").mock(
        return_value=httpx.Response(
            201,
            content=_envelope(
                {
                    "id": SCENE_ID,
                    "name": "Night Mode",
                    "description": "Dimmed",
                    "enabled": True,
                    "priority": 50,
                    "mutation_mode": "live",
                }
            ),
        )
    )

    scene = await client.create_scene("Night Mode", description="Dimmed")

    assert json.loads(route.calls[0].request.content) == {
        "name": "Night Mode",
        "description": "Dimmed",
    }
    assert scene.name == "Night Mode"


@respx.mock
@pytest.mark.asyncio
async def test_deactivate_scene(client: HypercolorClient) -> None:
    respx.post("http://hyperia.test:9420/api/v1/scenes/deactivate").mock(
        return_value=httpx.Response(
            200,
            content=_envelope(
                {
                    "deactivated": True,
                    "previous_scene": {
                        "id": SCENE_ID,
                        "name": "Battlestation",
                        "enabled": True,
                        "priority": 50,
                        "mutation_mode": "live",
                    },
                    "scene": None,
                }
            ),
        )
    )

    result = await client.deactivate_scene()

    assert result.deactivated is True
    assert result.previous_scene is not None
    assert result.previous_scene.name == "Battlestation"


@respx.mock
@pytest.mark.asyncio
async def test_get_zones(client: HypercolorClient) -> None:
    respx.get(f"http://hyperia.test:9420/api/v1/scenes/{SCENE_ID}/zones").mock(
        return_value=httpx.Response(
            200,
            content=_envelope({"items": [ZONE_PAYLOAD], "groups_revision": 12}),
        )
    )

    result = await client.get_zones(SCENE_ID)

    assert result.groups_revision == 12
    assert result.items[0].name == "Desk"
    assert result.items[0].layers[0].source["effect_id"] == "aurora"


@respx.mock
@pytest.mark.asyncio
async def test_create_zone_sends_if_match(client: HypercolorClient) -> None:
    route = respx.post(f"http://hyperia.test:9420/api/v1/scenes/{SCENE_ID}/zones").mock(
        return_value=httpx.Response(
            201,
            content=_envelope({"zone": ZONE_PAYLOAD, "groups_revision": 13}),
        )
    )

    result = await client.create_zone(SCENE_ID, "Desk", color="#e135ff", if_match=12)

    request = route.calls[0].request
    assert request.headers["if-match"] == '"12"'
    assert json.loads(request.content) == {"name": "Desk", "color": "#e135ff"}
    assert result.groups_revision == 13


@respx.mock
@pytest.mark.asyncio
async def test_stale_revision_raises_precondition_error(client: HypercolorClient) -> None:
    respx.post(f"http://hyperia.test:9420/api/v1/scenes/{SCENE_ID}/zones").mock(
        return_value=httpx.Response(
            412,
            headers={"ETag": '"14"'},
            content=msgspec.json.encode(
                {
                    "error": {
                        "code": "precondition_failed",
                        "message": "groups_revision mismatch",
                        "details": {},
                    },
                    "meta": {
                        "api_version": "1.0",
                        "request_id": "req_stale",
                        "timestamp": "2026-06-11T00:00:00Z",
                    },
                }
            ),
        )
    )

    with pytest.raises(HypercolorPreconditionError) as excinfo:
        await client.create_zone(SCENE_ID, "Desk", if_match=12)

    assert excinfo.value.status_code == 412
    assert excinfo.value.current_revision == 14


@respx.mock
@pytest.mark.asyncio
async def test_update_zone_distinguishes_clear_from_unset(client: HypercolorClient) -> None:
    route = respx.patch(f"http://hyperia.test:9420/api/v1/scenes/{SCENE_ID}/zones/{ZONE_ID}").mock(
        return_value=httpx.Response(
            200,
            content=_envelope({"zone": ZONE_PAYLOAD, "groups_revision": 13}),
        )
    )

    await client.update_zone(SCENE_ID, ZONE_ID, brightness=0.5, color=None, if_match=12)

    body = json.loads(route.calls[0].request.content)
    assert body == {"brightness": 0.5, "color": None}
    assert "description" not in body


@respx.mock
@pytest.mark.asyncio
async def test_delete_zone(client: HypercolorClient) -> None:
    respx.delete(f"http://hyperia.test:9420/api/v1/scenes/{SCENE_ID}/zones/{ZONE_ID}").mock(
        return_value=httpx.Response(
            200,
            content=_envelope({"zone_id": ZONE_ID, "deleted": True, "groups_revision": 14}),
        )
    )

    result = await client.delete_zone(SCENE_ID, ZONE_ID, if_match=13)

    assert result.deleted is True
    assert result.groups_revision == 14


@respx.mock
@pytest.mark.asyncio
async def test_assign_devices_normalizes_ids(client: HypercolorClient) -> None:
    route = respx.post(
        f"http://hyperia.test:9420/api/v1/scenes/{SCENE_ID}/zones/{ZONE_ID}/devices"
    ).mock(
        return_value=httpx.Response(
            200,
            content=_envelope({"items": [ZONE_PAYLOAD], "groups_revision": 15}),
        )
    )

    new_output = dict(ZONE_LAYOUT["zones"][0])
    result = await client.assign_devices(
        SCENE_ID, ZONE_ID, ["out-existing", new_output], if_match=14
    )

    body = json.loads(route.calls[0].request.content)
    assert body["device_zones"][0] == {"id": "out-existing"}
    assert body["device_zones"][1]["device_id"] == "usb:controller-1"
    assert result.groups_revision == 15


@respx.mock
@pytest.mark.asyncio
async def test_set_unassigned_behavior_accepts_fallback(client: HypercolorClient) -> None:
    route = respx.patch(
        f"http://hyperia.test:9420/api/v1/scenes/{SCENE_ID}/unassigned-behavior"
    ).mock(
        return_value=httpx.Response(
            200,
            content=_envelope(
                {"unassigned_behavior": {"fallback": ZONE_ID}, "groups_revision": 16}
            ),
        )
    )

    result = await client.set_unassigned_behavior(SCENE_ID, {"fallback": ZONE_ID}, if_match=15)

    assert json.loads(route.calls[0].request.content) == {
        "unassigned_behavior": {"fallback": ZONE_ID}
    }
    assert result.groups_revision == 16


@respx.mock
@pytest.mark.asyncio
async def test_get_active_layout_decodes_spatial_layout(client: HypercolorClient) -> None:
    respx.get("http://hyperia.test:9420/api/v1/layouts/active").mock(
        return_value=httpx.Response(200, content=_envelope(ZONE_LAYOUT))
    )

    layout = await client.get_active_layout()

    assert layout is not None
    assert layout.canvas_width == 320
    assert layout.zones[0].zone_name == "atx"
    assert layout.zones[0].led_count == 24
