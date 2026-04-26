"""Model conversion tests."""

from __future__ import annotations

import msgspec

from hypercolor.models.device import Device
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
