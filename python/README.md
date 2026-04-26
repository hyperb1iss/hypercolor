# Hypercolor Python

Async Python client and WebSocket helpers for the Hypercolor daemon.

This package lives inside the main Hypercolor repository at `python/`. It is a
standalone uv project, so Python contributors can work here without touching the
Rust workspace.

Generation details live in `../docs/development/CLIENT_GENERATION.md`.

## Tooling

```bash
uv sync
uv run ruff check .
uv run ruff format --check .
uv run ty check
uv run python scripts/generate_ws_protocol.py --check
uv run pytest
uv run python scripts/generate_openapi_client.py --check
```

Or use the local recipes:

```bash
just verify
just fix
just generate
```

## Library

```python
from hypercolor import HypercolorClient

async with HypercolorClient() as client:
    status = await client.get_status()
    devices = await client.get_devices()
```
