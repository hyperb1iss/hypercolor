# Hypercolor Python

Async Python client and WebSocket helpers for the Hypercolor daemon.

This package lives inside the main Hypercolor repository at `python/`. It is a
standalone uv project, so Python contributors can work here without touching the
Rust workspace.

Generation details live in `../docs/development/CLIENT_GENERATION.md`.

## Install

```bash
uv sync
```

Once the package is published, apps should depend on it normally:

```bash
uv add hypercolor
```

## Quick Start

```python
import asyncio

from hypercolor import HypercolorClient


async def main() -> None:
    async with HypercolorClient() as client:
        status = await client.get_status()
        devices = await client.get_devices()

    print(status.running)
    print([device.name for device in devices])


asyncio.run(main())
```

The async client is the primary API. Use `SyncHypercolorClient` for scripts or
small tools that are not already running an event loop.

```python
from hypercolor import SyncHypercolorClient

with SyncHypercolorClient() as client:
    print(client.get_status().global_brightness)
```

## Drivers

Driver inventory includes module capabilities, config keys, optional control
surface paths, and protocol catalogs for HAL-backed modules.

```python
import asyncio

from hypercolor import HypercolorClient


async def main() -> None:
    async with HypercolorClient() as client:
        for driver in await client.get_drivers():
            if driver.descriptor.capabilities.protocol_catalog:
                print(driver.descriptor.display_name)
                print([protocol.display_name for protocol in driver.protocols])


asyncio.run(main())
```

## Effects

```python
import asyncio

from hypercolor import HypercolorClient


async def main() -> None:
    async with HypercolorClient() as client:
        effects = await client.get_effects()
        aurora = next(effect for effect in effects if effect.name == "Aurora")

        await client.apply_effect(
            aurora.id,
            controls={"speed": 72, "palette": "silkcircuit"},
            transition={"type": "fade", "duration_ms": 400},
        )


asyncio.run(main())
```

## Control Surfaces

Control surfaces are dynamic device and driver settings. The generated OpenAPI
models stay private; the public client accepts normal Python values.

```python
import asyncio

from hypercolor import HypercolorClient


async def main() -> None:
    async with HypercolorClient() as client:
        surface = await client.get_device_controls("keyboard")

        await client.set_control_values(
            surface.id,
            {
                "enabled": True,
                "brightness": 88,
            },
            expected_revision=surface.revision,
        )

        await client.invoke_control_action(
            surface.id,
            "identify",
            {"duration_ms": 750},
        )


asyncio.run(main())
```

Inside an async function, typed daemon values can pass through directly:

```python
async with HypercolorClient() as client:
    await client.set_control_values(
        "device:keyboard",
        {"accent": {"kind": "color_rgb", "value": [128, 255, 234]}},
    )
```

## WebSocket Events

```python
import asyncio

from hypercolor import HypercolorClient
from hypercolor.websocket import EventMessage, MetricsMessage


async def main() -> None:
    async with HypercolorClient() as client:
        async with client.events() as stream:
            await stream.subscribe("events", "metrics")

            async for message in stream:
                if isinstance(message, EventMessage):
                    print(message.event, message.data)
                elif isinstance(message, MetricsMessage):
                    print(message.data)


asyncio.run(main())
```

Binary frame, spectrum, and canvas messages are decoded into dataclasses, so
callers do not need to parse the wire format.

## Development

Use the local recipes from `python/`:

```bash
just verify
just fix
just generate
just generate-check
```

The full project recipe also runs the Python gate:

```bash
just python-verify
```

Manual equivalents:

```bash
uv run ruff check .
uv run ruff format --check .
uv run ty check
uv run python scripts/generate_ws_protocol.py --check
uv run pytest
uv run python scripts/generate_openapi_client.py --check
```
