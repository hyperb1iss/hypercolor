"""Model conversion tests."""

from __future__ import annotations

import msgspec

from hypercolor.models.device import Device
from hypercolor.models.driver import Driver
from hypercolor.models.effect import Effect


def test_device_model_decodes() -> None:
    payload = {
        "id": "keyboard",
        "layout_device_id": "keyboard",
        "name": "Keyboard",
        "backend": "hid",
        "status": "connected",
        "brightness": 92,
        "firmware_version": "1.2.3",
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
    }

    device = msgspec.convert(payload, type=Device)

    assert device.name == "Keyboard"
    assert device.connection_label == "USB HID"
    assert device.zones[0].topology == "matrix"
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
        "zones": [
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

    assert device.backend == "wled"
    assert device.driver_id == "wled"
    assert device.transport == "network"
    assert device.connection_label == "wled-studio.local"
    assert device.network_ip == "10.4.22.169"
    assert device.network_hostname == "wled-studio.local"
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
                "label": "Animation Speed",
                "type": "number",
                "min": 0,
                "max": 100,
                "step": 1,
                "default": 40,
            }
        ],
        "presets": [{"name": "Default", "is_default": True}],
        "active_control_values": {"effectSpeed": 70},
    }

    effect = msgspec.convert(payload, type=Effect)

    assert effect.id == "aurora"
    assert effect.active_control_values == {"effectSpeed": 70}
    assert effect.presets[0].is_default is True


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
