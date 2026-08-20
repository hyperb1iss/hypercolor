"""Model conversion tests."""

from __future__ import annotations

import msgspec

from hypercolor._generated.api.devices import update_attachments
from hypercolor._generated.models import (
    ComponentBinding,
    EffectDetailResponse,
    UpdateAttachmentsRequest,
)
from hypercolor._generated.types import Unset
from hypercolor.models.device import Device
from hypercolor.models.driver import Driver


def test_generated_attachment_update_sends_the_complete_request() -> None:
    kwargs = update_attachments._get_kwargs(
        "desk/controller",
        body=UpdateAttachmentsRequest(
            bindings=[
                ComponentBinding(
                    slot_id="channel-1",
                    template_id="strip-60",
                    instances=2,
                )
            ],
            validate_only=True,
        ),
    )

    assert kwargs["url"] == "/api/v1/devices/desk%2Fcontroller/attachments"
    assert kwargs["json"] == {
        "bindings": [
            {
                "slot_id": "channel-1",
                "template_id": "strip-60",
                "instances": 2,
            }
        ],
        "validate_only": True,
    }


def test_device_model_decodes_canonical_connection() -> None:
    payload = {
        "id": "keyboard",
        "layout_device_id": "keyboard",
        "name": "Keyboard",
        "origin": {"driver_id": "hid", "backend_id": "hid", "transport": "usb"},
        "status": "connected",
        "brightness": 92,
        "firmware_version": "1.2.3",
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

    device = msgspec.convert(payload, type=Device)

    assert device.name == "Keyboard"
    assert device.connection is not None
    assert device.connection.label == "USB HID"
    assert device.segments[0].topology == "matrix"
    assert device.enabled is True


def test_device_model_decodes_current_daemon_shape() -> None:
    payload = {
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

    device = msgspec.convert(payload, type=Device)

    assert device.driver_id == "wled"
    assert device.transport == "network"
    assert device.origin is not None
    assert device.origin.backend_id == "wled"
    assert device.connection is not None
    assert device.connection.endpoint == "wled-studio.local"
    assert device.connection.ip == "10.4.22.169"
    assert device.connection.hostname == "wled-studio.local"
    assert device.presentation is not None
    assert device.presentation.label == "WLED"


def test_effect_model_decodes() -> None:
    payload = {
        "id": "aurora",
        "name": "Aurora",
        "description": "Northern lights",
        "author": "SignalRGB",
        "category": "ambient",
        "source": "native",
        "runnable": True,
        "tags": ["nature", "gradient"],
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
        "presets": [
            {
                "id": "default",
                "name": "Default",
                "controls": {"effectSpeed": {"integer": 40}},
            }
        ],
    }

    effect = EffectDetailResponse.from_dict(payload)

    assert effect.id == "aurora"
    assert not isinstance(effect.controls, Unset)
    assert effect.controls[0].name == "Animation Speed"
    assert not isinstance(effect.presets, Unset)
    assert effect.presets[0].id == "default"


def test_driver_model_decodes_protocol_catalog() -> None:
    payload = {
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
                "presentation": {"label": "Nollie 8", "icon": "grid"},
            }
        ],
    }

    driver = msgspec.convert(payload, type=Driver)

    assert driver.descriptor.capabilities.protocol_catalog is True
    assert driver.presentation is not None
    assert driver.presentation.label == "Nollie"
    assert driver.protocols[0].protocol_id == "nollie_8"
    assert driver.protocols[0].vendor_id == 0x2E8A
