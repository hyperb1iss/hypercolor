"""Tests for the canonical live scene-tree client surface."""

from __future__ import annotations

import json

import httpx
import msgspec
import pytest
import respx

from hypercolor.client import HypercolorClient
from hypercolor.exceptions import HypercolorPreconditionError

SCENE_ID = "0193d2c0-0000-7000-8000-00000000aaaa"
ZONE_ID = "0193d2c0-0000-7000-8000-000000000001"
LAYER_ID = "0193d2c0-0000-7000-8000-000000000002"

ZONE = {
    "id": ZONE_ID,
    "name": "Desk",
    "role": "primary",
    "enabled": True,
    "brightness": 0.8,
    "color": "#e135ff",
    "display_target": None,
    "members": [
        {
            "id": "out-strimer",
            "device_id": "usb:controller-1",
            "segment": "atx",
            "name": "ATX Strimer",
        }
    ],
    "layout": {
        "placements": [
            {
                "member": "out-strimer",
                "position": {"x": 0.5, "y": 0.5},
                "size": {"x": 0.4, "y": 0.2},
                "rotation": 0.0,
                "scale": 1.0,
                "orientation": "horizontal",
                "topology": {"type": "strip", "count": 24, "direction": "left_to_right"},
            }
        ]
    },
    "layers": [
        {
            "id": LAYER_ID,
            "source": {
                "type": "effect",
                "effect_id": "aurora",
                "controls": {"speed": {"integer": 50}},
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
}

LIVE_SCENE = {
    "id": SCENE_ID,
    "name": "Battlestation",
    "kind": "named",
    "is_default": False,
    "unassigned_behavior": "off",
    "layout_id": None,
    "revision": 12,
    "zones": [ZONE],
}


def _envelope(data: object) -> bytes:
    return msgspec.json.encode(
        {
            "data": data,
            "meta": {
                "api_version": "1.0",
                "request_id": "req_scene",
                "timestamp": "2026-08-19T00:00:00Z",
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
async def test_get_live_scene_uses_canonical_singleton(client: HypercolorClient) -> None:
    respx.get("http://hyperia.test:9420/api/v1/scene").mock(
        return_value=httpx.Response(200, content=_envelope(LIVE_SCENE))
    )

    scene = await client.get_live_scene()

    assert scene.revision == 12
    assert scene.zones[0].layers[0].id == LAYER_ID


@respx.mock
@pytest.mark.asyncio
async def test_patch_live_scene_uses_one_revision_token(client: HypercolorClient) -> None:
    route = respx.patch("http://hyperia.test:9420/api/v1/scene").mock(
        return_value=httpx.Response(200, content=_envelope(LIVE_SCENE))
    )

    await client.patch_live_scene(
        unassigned_behavior={"fallback": ZONE_ID},
        if_match=11,
    )

    request = route.calls[0].request
    assert request.headers["if-match"] == '"11"'
    assert json.loads(request.content) == {"unassigned_behavior": {"fallback": ZONE_ID}}


@respx.mock
@pytest.mark.asyncio
async def test_deactivate_scene_uses_live_tree_route(client: HypercolorClient) -> None:
    route = respx.post("http://hyperia.test:9420/api/v1/scene/deactivate").mock(
        return_value=httpx.Response(200, content=_envelope(LIVE_SCENE))
    )

    scene = await client.deactivate_scene()

    assert route.called
    assert scene.id == SCENE_ID


@respx.mock
@pytest.mark.asyncio
async def test_clear_scene_can_target_one_zone(client: HypercolorClient) -> None:
    route = respx.post("http://hyperia.test:9420/api/v1/scene/clear").mock(
        return_value=httpx.Response(200, content=_envelope(LIVE_SCENE))
    )

    scene = await client.clear_scene(zone=ZONE_ID, if_match=12)

    request = route.calls[0].request
    assert request.headers["if-match"] == '"12"'
    assert json.loads(request.content) == {"zone": ZONE_ID}
    assert scene.revision == 12


@respx.mock
@pytest.mark.asyncio
async def test_clear_scene_omits_body_when_clearing_every_zone(
    client: HypercolorClient,
) -> None:
    route = respx.post("http://hyperia.test:9420/api/v1/scene/clear").mock(
        return_value=httpx.Response(200, content=_envelope(LIVE_SCENE))
    )

    await client.clear_scene()

    request = route.calls[0].request
    assert request.content == b""
    assert "content-type" not in request.headers


@respx.mock
@pytest.mark.asyncio
async def test_get_zone_uses_live_tree_identity(client: HypercolorClient) -> None:
    route = respx.get(f"http://hyperia.test:9420/api/v1/scene/zones/{ZONE_ID}").mock(
        return_value=httpx.Response(200, content=_envelope(ZONE))
    )

    zone = await client.get_zone(ZONE_ID)

    assert route.called
    assert zone.members[0].id == "out-strimer"


@respx.mock
@pytest.mark.asyncio
async def test_create_zone_uses_live_tree_route(client: HypercolorClient) -> None:
    route = respx.post("http://hyperia.test:9420/api/v1/scene/zones").mock(
        return_value=httpx.Response(201, content=_envelope(ZONE))
    )

    zone = await client.create_zone(
        "Desk",
        role="custom",
        color="#e135ff",
        if_match=12,
    )

    request = route.calls[0].request
    assert request.headers["if-match"] == '"12"'
    assert json.loads(request.content) == {
        "name": "Desk",
        "role": "custom",
        "color": "#e135ff",
    }
    assert zone.id == ZONE_ID


@respx.mock
@pytest.mark.asyncio
async def test_stale_revision_raises_precondition_error(client: HypercolorClient) -> None:
    respx.post("http://hyperia.test:9420/api/v1/scene/zones").mock(
        return_value=httpx.Response(
            412,
            headers={"ETag": '"14"'},
            content=msgspec.json.encode(
                {
                    "error": {
                        "code": "precondition_failed",
                        "message": "version mismatch: expected 12, current 14",
                        "details": {"expected": 12, "current": 14},
                    },
                    "meta": {
                        "api_version": "1.0",
                        "request_id": "req_stale",
                        "timestamp": "2026-08-19T00:00:00Z",
                    },
                }
            ),
        )
    )

    with pytest.raises(HypercolorPreconditionError) as excinfo:
        await client.create_zone("Desk", if_match=12)

    assert excinfo.value.current_revision == 14


@respx.mock
@pytest.mark.asyncio
async def test_update_zone_distinguishes_clear_from_unset(client: HypercolorClient) -> None:
    route = respx.patch(f"http://hyperia.test:9420/api/v1/scene/zones/{ZONE_ID}").mock(
        return_value=httpx.Response(200, content=_envelope(ZONE))
    )

    await client.update_zone(ZONE_ID, brightness=0.5, color=None, if_match=12)

    assert json.loads(route.calls[0].request.content) == {
        "brightness": 0.5,
        "color": None,
    }


@respx.mock
@pytest.mark.asyncio
async def test_delete_zone_uses_live_tree_route(client: HypercolorClient) -> None:
    route = respx.delete(f"http://hyperia.test:9420/api/v1/scene/zones/{ZONE_ID}").mock(
        return_value=httpx.Response(200, content=_envelope(LIVE_SCENE))
    )

    scene = await client.delete_zone(ZONE_ID, if_match=13)

    assert route.calls[0].request.headers["if-match"] == '"13"'
    assert scene.revision == 12


@respx.mock
@pytest.mark.asyncio
async def test_assign_members_uses_device_and_segments(client: HypercolorClient) -> None:
    route = respx.post(f"http://hyperia.test:9420/api/v1/scene/zones/{ZONE_ID}/members").mock(
        return_value=httpx.Response(200, content=_envelope(ZONE))
    )

    zone = await client.assign_members(
        ZONE_ID,
        "usb:controller-1",
        segments=["atx"],
        if_match=14,
    )

    assert json.loads(route.calls[0].request.content) == {
        "device_id": "usb:controller-1",
        "segments": ["atx"],
    }
    assert zone.members[0].segment == "atx"


@respx.mock
@pytest.mark.asyncio
async def test_unassign_member_addresses_membership_id(client: HypercolorClient) -> None:
    route = respx.delete(
        f"http://hyperia.test:9420/api/v1/scene/zones/{ZONE_ID}/members/out-strimer"
    ).mock(return_value=httpx.Response(200, content=_envelope(ZONE)))

    await client.unassign_member(ZONE_ID, "out-strimer", if_match=15)

    assert route.calls[0].request.headers["if-match"] == '"15"'


@respx.mock
@pytest.mark.asyncio
async def test_set_zone_layout_uses_compact_placements(client: HypercolorClient) -> None:
    route = respx.put(f"http://hyperia.test:9420/api/v1/scene/zones/{ZONE_ID}/layout").mock(
        return_value=httpx.Response(200, content=_envelope(ZONE))
    )
    layout = ZONE["layout"]
    assert isinstance(layout, dict)

    zone = await client.set_zone_layout(ZONE_ID, layout, if_match=16)

    assert json.loads(route.calls[0].request.content) == layout
    assert zone.layout == layout


@respx.mock
@pytest.mark.asyncio
async def test_set_unassigned_behavior_patches_live_scene(client: HypercolorClient) -> None:
    route = respx.patch("http://hyperia.test:9420/api/v1/scene").mock(
        return_value=httpx.Response(200, content=_envelope(LIVE_SCENE))
    )

    await client.set_unassigned_behavior({"fallback": ZONE_ID}, if_match=17)

    assert json.loads(route.calls[0].request.content) == {
        "unassigned_behavior": {"fallback": ZONE_ID}
    }
