# Hypercolor Python

Async Python client, CLI, and WebSocket helpers for the Hypercolor daemon.

This package lives inside the main Hypercolor repository at `python/`. It is a
standalone uv project, so Python contributors can work here without touching the
Rust workspace.

## Tooling

```bash
uv sync
uv run ruff check .
uv run ruff format --check .
uv run ty check
uv run pytest
```

Or use the local recipes:

```bash
just verify
just fix
```

## CLI

```bash
uv run hypercolor status
uv run hypercolor device list
uv run hypercolor effect list
```

## Library

```python
from hypercolor import HypercolorClient

async with HypercolorClient() as client:
    status = await client.get_status()
    devices = await client.get_devices()
```
