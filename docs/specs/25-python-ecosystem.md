# Spec 25 -- Python Ecosystem & Home Assistant Integration

> Three-project stack: async Python client, Home Assistant integration, and Lovelace card — all built on modern 2026 tooling.

**Status:** Draft
**Date:** 2026-03-08
**Projects:** `hypercolor-python`, `hypercolor-homeassistant`, `hypercolor-card`
**Reference:** Existing SignalRGB stack (`signalrgb-python`, `signalrgb-homeassistant`, `hyper-light-card`)

---

## Table of Contents

1. [Overview](#1-overview)
2. [Architecture](#2-architecture)
3. [hypercolor-python](#3-hypercolor-python)
4. [hypercolor-homeassistant](#4-hypercolor-homeassistant)
5. [hypercolor-card](#5-hypercolor-card)
6. [Tooling & Quality](#6-tooling--quality)
7. [CI/CD](#7-cicd)
8. [Implementation Plan](#8-implementation-plan)
9. [Open Questions](#9-open-questions)
10. [Recommendation](#10-recommendation)

---

## 1. Overview

### The Stack

| Project | Repo | Purpose |
|---------|------|---------|
| **hypercolor-python** | `~/dev/hypercolor-python` | Async Python API client + CLI (`hyper` command) |
| **hypercolor-homeassistant** | `~/dev/hypercolor-homeassistant` | HA custom integration (HACS-compatible) |
| **hypercolor-card** | `~/dev/hypercolor-card` | Custom Lovelace card with effect visualization |

### What We're Wrapping

Hypercolor daemon API on `:9420` — REST + WebSocket with:
- ~60 REST endpoints across 8 domains (devices, effects, layouts, scenes, profiles, library, attachments, system)
- WebSocket with 5 channels (events, frames, spectrum, canvas, metrics)
- Binary frame protocol for LED data + audio FFT
- Optional Bearer token auth (when network-exposed)
- Standard JSON envelope: `{ data, meta: { api_version, request_id, timestamp } }`

### What's Different From SignalRGB

| Aspect | SignalRGB | Hypercolor |
|--------|-----------|------------|
| API surface | ~15 endpoints | ~60 endpoints |
| WebSocket | None | 5 channels, binary frames |
| Auth | None | Bearer tokens (optional) |
| Response format | Custom `{ status, api_version }` | Standard envelope `{ data, meta }` |
| Device model | Single canvas | Multi-device, zones, layouts |
| Effects | Simple list + apply | Controls schema, presets, playlists |
| Scenes | None | Scene engine with triggers |
| Profiles | None | Full system snapshots |
| Audio | None | FFT spectrum, beat detection |

---

## 2. Architecture

```
┌─────────────────────────────────────┐
│          hypercolor-card            │  Lovelace card (Lit 3, Vite)
│  Effect visualization + controls   │
└──────────────┬──────────────────────┘
               │ HA WebSocket (via custom-card-helpers)
┌──────────────▼──────────────────────┐
│     hypercolor-homeassistant        │  HA Custom Integration
│  Light + Select + Sensor + Button   │
│  DataUpdateCoordinator + Events     │
└──────────────┬──────────────────────┘
               │ Python API
┌──────────────▼──────────────────────┐
│        hypercolor-python            │  Async Client (httpx + websockets)
│  Models + Client + CLI + WS        │
└──────────────┬──────────────────────┘
               │ HTTP/WS
┌──────────────▼──────────────────────┐
│       hypercolor-daemon :9420       │  Rust (Axum)
│  REST + WebSocket + Binary Frames   │
└─────────────────────────────────────┘
```

---

## 3. hypercolor-python

### 3.1 Project Setup

```
hypercolor-python/
├── src/
│   └── hypercolor/
│       ├── __init__.py          # Public API exports
│       ├── client.py            # HypercolorClient (async, primary)
│       ├── sync_client.py       # SyncHypercolorClient (sync wrapper)
│       ├── websocket.py         # WebSocket client + binary frame parsing
│       ├── models/
│       │   ├── __init__.py
│       │   ├── device.py        # Device, DeviceState, DeviceFamily, Zone
│       │   ├── effect.py        # Effect, ControlDefinition, ControlValue
│       │   ├── layout.py        # SpatialLayout, LedTopology
│       │   ├── scene.py         # Scene, SceneTrigger, TransitionSpec
│       │   ├── profile.py       # Profile
│       │   ├── library.py       # Preset, Playlist, PlaylistItem, Favorite
│       │   ├── attachment.py    # AttachmentTemplate, AttachmentBinding
│       │   ├── audio.py         # AudioDevice, SpectrumData, BeatInfo
│       │   ├── system.py        # SystemStatus, HealthCheck, Metrics
│       │   └── common.py        # ApiEnvelope, ApiError, Meta, Pagination
│       ├── exceptions.py        # Exception hierarchy
│       ├── constants.py         # Defaults, endpoints
│       └── cli/
│           ├── __init__.py
│           ├── app.py           # Typer app entry
│           ├── commands/
│           │   ├── device.py
│           │   ├── effect.py
│           │   ├── layout.py
│           │   ├── scene.py
│           │   ├── profile.py
│           │   └── system.py
│           └── formatting.py    # Rich tables, SilkCircuit colors
├── tests/
│   ├── conftest.py
│   ├── data/                    # JSON fixtures
│   ├── test_client.py
│   ├── test_sync_client.py
│   ├── test_websocket.py
│   └── test_models/
│       ├── test_device.py
│       ├── test_effect.py
│       └── ...
├── docs/
├── pyproject.toml
└── .github/workflows/ci-cd.yml
```

**Key difference from signalrgb-python:** Uses `src/` layout (PEP 660 standard), `msgspec` instead of `mashumaro` (faster, zero-copy), and a modular models directory.

### 3.2 Tooling

| Tool | Version | Purpose |
|------|---------|---------|
| **uv** | latest | Project management, venv, lockfile |
| **ruff** | latest | Lint + format (replaces black, isort, flake8, pylint) |
| **ty** | latest | Type checking (replaces mypy — faster, Astral-native) |
| **pytest** | 8.x | Testing |
| **pytest-asyncio** | latest | Async test support |

**Build system:** `hatchling` (simple, well-supported)

**Runtime deps:**
- `httpx>=0.28` — async HTTP client
- `websockets>=14` — async WebSocket client (for WS channels)
- `msgspec>=0.19` — zero-copy JSON/binary (de)serialization
- `typer>=0.16` — CLI framework
- `rich>=14` — terminal formatting

**Dev deps:**
- `pytest`, `pytest-asyncio`, `pytest-cov`
- `respx` — httpx mocking (better than unittest.mock for httpx)

### 3.3 Models (msgspec Structs)

msgspec over mashumaro — it's faster, supports zero-copy binary decode, and has built-in validation. Perfect for binary WebSocket frames.

```python
import msgspec

class Meta(msgspec.Struct):
    api_version: str
    request_id: str
    timestamp: str

class ApiEnvelope[T](msgspec.Struct, Generic[T]):
    """Standard response wrapper."""
    data: T
    meta: Meta

class ApiError(msgspec.Struct):
    code: str
    message: str
    details: dict[str, Any] | None = None

class ErrorEnvelope(msgspec.Struct):
    error: ApiError
    meta: Meta
```

**Device models:**
```python
class DeviceInfo(msgspec.Struct, rename="camel"):
    id: str
    name: str
    vendor: str
    product: str
    family: str
    backend: str
    led_count: int
    zones: list[Zone]
    state: DeviceState
    enabled: bool = True

class Zone(msgspec.Struct, rename="camel"):
    id: str
    name: str
    led_count: int
    zone_type: str

class DeviceState(msgspec.Struct, rename="camel"):
    connected: bool
    last_seen: str | None = None
    error: str | None = None
```

**Effect models:**
```python
class Effect(msgspec.Struct, rename="camel"):
    id: str
    name: str
    description: str | None = None
    category: str | None = None
    author: str | None = None
    controls: list[ControlDefinition] = []
    tags: list[str] = []
    preview_url: str | None = None

class ControlDefinition(msgspec.Struct, rename="camel"):
    id: str
    name: str
    control_type: str  # "range" | "color" | "boolean" | "select" | "palette"
    default: Any = None
    min: float | None = None
    max: float | None = None
    step: float | None = None
    options: list[str] | None = None

class ActiveEffect(msgspec.Struct, rename="camel"):
    effect: Effect
    controls: dict[str, Any]
    layout_id: str | None = None
```

*Full models for Layout, Scene, Profile, Library, Attachment, Audio follow same pattern.*

### 3.4 Exception Hierarchy

```python
class HypercolorError(Exception):
    """Base exception for all Hypercolor errors."""
    def __init__(self, message: str, error: ApiError | None = None):
        super().__init__(message)
        self.error = error

    @property
    def code(self) -> str | None:
        return self.error.code if self.error else None

class ConnectionError(HypercolorError):
    """Failed to connect to daemon."""

class AuthenticationError(HypercolorError):
    """Invalid or missing API key."""

class NotFoundError(HypercolorError):
    """Resource not found (404)."""

class ValidationError(HypercolorError):
    """Request validation failed (422)."""

class RateLimitError(HypercolorError):
    """Rate limit exceeded (429)."""
    def __init__(self, message: str, retry_after: float, **kwargs):
        super().__init__(message, **kwargs)
        self.retry_after = retry_after

class ConflictError(HypercolorError):
    """Resource conflict (409)."""

class ApiError(HypercolorError):
    """Generic API error."""
```

### 3.5 Async Client

```python
class HypercolorClient:
    """Async client for the Hypercolor daemon API."""

    def __init__(
        self,
        host: str = DEFAULT_HOST,
        port: int = DEFAULT_PORT,
        api_key: str | None = None,
        timeout: float = DEFAULT_TIMEOUT,
    ) -> None:
        self._base_url = f"http://{host}:{port}/api/v1"
        self._ws_url = f"ws://{host}:{port}/api/v1/ws"
        self._timeout = timeout
        self._api_key = api_key
        self._client = httpx.AsyncClient(
            base_url=self._base_url,
            timeout=self._timeout,
            headers=self._auth_headers(),
        )

    # --- Context manager ---
    async def __aenter__(self) -> Self: ...
    async def __aexit__(self, *exc) -> None: await self.aclose()

    # --- Devices ---
    async def get_devices(self, **filters) -> list[DeviceInfo]: ...
    async def get_device(self, device_id: str) -> DeviceInfo: ...
    async def update_device(self, device_id: str, **kwargs) -> DeviceInfo: ...
    async def remove_device(self, device_id: str) -> None: ...
    async def discover_devices(self, backends: list[str] | None = None) -> DiscoverResult: ...
    async def identify_device(self, device_id: str) -> None: ...

    # --- Effects ---
    async def get_effects(self) -> list[Effect]: ...
    async def get_effect(self, effect_id: str) -> Effect: ...
    async def apply_effect(
        self,
        effect_id: str,
        controls: dict[str, Any] | None = None,
        transition: TransitionSpec | None = None,
    ) -> ApplyResult: ...
    async def get_active_effect(self) -> ActiveEffect | None: ...
    async def update_controls(self, controls: dict[str, Any]) -> ControlUpdateResult: ...
    async def reset_controls(self) -> None: ...
    async def stop_effect(self) -> None: ...

    # --- Layouts ---
    async def get_layouts(self) -> list[SpatialLayout]: ...
    async def get_layout(self, layout_id: str) -> SpatialLayout: ...
    async def get_active_layout(self) -> SpatialLayout | None: ...
    async def create_layout(self, layout: CreateLayoutRequest) -> SpatialLayout: ...
    async def update_layout(self, layout_id: str, **kwargs) -> SpatialLayout: ...
    async def delete_layout(self, layout_id: str) -> None: ...
    async def apply_layout(self, layout_id: str) -> None: ...

    # --- Scenes ---
    async def get_scenes(self, **filters) -> list[Scene]: ...
    async def get_scene(self, scene_id: str) -> Scene: ...
    async def create_scene(self, scene: CreateSceneRequest) -> Scene: ...
    async def update_scene(self, scene_id: str, **kwargs) -> Scene: ...
    async def delete_scene(self, scene_id: str) -> None: ...
    async def activate_scene(self, scene_id: str) -> None: ...

    # --- Profiles ---
    async def get_profiles(self) -> list[Profile]: ...
    async def get_profile(self, profile_id: str) -> Profile: ...
    async def create_profile(self, profile: CreateProfileRequest) -> Profile: ...
    async def update_profile(self, profile_id: str, **kwargs) -> Profile: ...
    async def delete_profile(self, profile_id: str) -> None: ...
    async def apply_profile(self, profile_id: str) -> None: ...

    # --- Library ---
    async def get_favorites(self) -> list[str]: ...
    async def add_favorite(self, effect_id: str) -> None: ...
    async def remove_favorite(self, effect_id: str) -> None: ...
    async def get_presets(self) -> list[Preset]: ...
    async def get_preset(self, preset_id: str) -> Preset: ...
    async def save_preset(self, preset: CreatePresetRequest) -> Preset: ...
    async def apply_preset(self, preset_id: str) -> None: ...
    async def delete_preset(self, preset_id: str) -> None: ...
    async def get_playlists(self) -> list[Playlist]: ...
    async def get_playlist(self, playlist_id: str) -> Playlist: ...
    async def create_playlist(self, playlist: CreatePlaylistRequest) -> Playlist: ...
    async def activate_playlist(self, playlist_id: str) -> None: ...
    async def stop_playlist(self) -> None: ...

    # --- System ---
    async def get_status(self) -> SystemStatus: ...
    async def health(self) -> bool: ...
    async def get_audio_devices(self) -> list[AudioDevice]: ...

    # --- WebSocket ---
    def events(self) -> HypercolorEventStream: ...
```

**Request internals:**

```python
async def _request(
    self,
    method: str,
    path: str,
    *,
    body: Any | None = None,
    params: dict[str, Any] | None = None,
    response_type: type[T] = dict,
) -> T:
    """Send request, unwrap envelope, return typed data."""
    try:
        resp = await self._client.request(method, path, json=body, params=params)
        resp.raise_for_status()
        envelope = msgspec.json.decode(resp.content, type=ApiEnvelope[response_type])
        return envelope.data
    except httpx.ConnectError:
        raise ConnectionError("Failed to connect to Hypercolor daemon")
    except httpx.TimeoutException:
        raise ConnectionError("Request timed out")
    except httpx.HTTPStatusError as e:
        self._raise_for_status(e)
```

### 3.6 WebSocket Client

The big upgrade over signalrgb-python — full WebSocket support with binary frame parsing.

```python
class HypercolorEventStream:
    """WebSocket connection with channel subscriptions."""

    def __init__(self, client: HypercolorClient):
        self._url = client._ws_url
        self._api_key = client._api_key
        self._ws: websockets.WebSocketClientProtocol | None = None
        self._subscriptions: set[str] = {"events"}
        self._reconnect_delay = 0.5
        self._max_reconnect_delay = 30.0
        self._handlers: dict[str, list[Callable]] = {}

    async def connect(self) -> None: ...
    async def disconnect(self) -> None: ...

    # --- Subscriptions ---
    async def subscribe(
        self,
        *channels: str,
        config: dict[str, Any] | None = None,
    ) -> None:
        """Subscribe to channels: 'events', 'frames', 'spectrum', 'canvas', 'metrics'."""
        ...

    async def unsubscribe(self, *channels: str) -> None: ...

    # --- Event handling ---
    def on(self, event: str, handler: Callable) -> None:
        """Register handler for event type (e.g. 'effect_changed')."""
        ...

    def on_frames(self, handler: Callable[[FrameData], Any]) -> None:
        """Register handler for binary LED frame data."""
        ...

    def on_spectrum(self, handler: Callable[[SpectrumData], Any]) -> None:
        """Register handler for audio spectrum data."""
        ...

    def on_metrics(self, handler: Callable[[MetricsSnapshot], Any]) -> None:
        """Register handler for performance metrics."""
        ...

    # --- Bidirectional commands ---
    async def command(
        self,
        method: str,
        path: str,
        body: Any | None = None,
    ) -> dict[str, Any]:
        """Send REST-equivalent command over WebSocket."""
        ...

    # --- Binary frame parsing ---
    @staticmethod
    def _parse_led_frame(data: bytes) -> FrameData: ...

    @staticmethod
    def _parse_spectrum(data: bytes) -> SpectrumData: ...

    @staticmethod
    def _parse_canvas(data: bytes) -> CanvasData: ...

    # --- Async iteration ---
    async def __aiter__(self) -> AsyncIterator[Event]: ...

    # --- Context manager ---
    async def __aenter__(self) -> Self: ...
    async def __aexit__(self, *exc) -> None: ...
```

**Usage:**
```python
async with HypercolorClient("hyperia.home") as client:
    async with client.events() as stream:
        await stream.subscribe("events", "spectrum")

        stream.on("effect_changed", lambda e: print(f"Now playing: {e.data['name']}"))
        stream.on_spectrum(lambda s: print(f"Bass: {s.bass:.1f}"))

        async for event in stream:
            ...  # Process events
```

### 3.7 Sync Client

Same adapter pattern as signalrgb-python, but simplified — Python 3.13 has better `asyncio.Runner`:

```python
class SyncHypercolorClient:
    """Synchronous wrapper around HypercolorClient."""

    def __init__(self, host: str = DEFAULT_HOST, port: int = DEFAULT_PORT, **kwargs):
        self._runner = asyncio.Runner()
        self._async_client = HypercolorClient(host, port, **kwargs)

    def __enter__(self) -> Self:
        self._runner.run(self._async_client.__aenter__())
        return self

    def __exit__(self, *exc) -> None:
        self._runner.run(self._async_client.__aexit__(*exc))
        self._runner.close()

    # Every async method gets a sync twin via __getattr__ or explicit delegation
    def get_devices(self, **kwargs) -> list[DeviceInfo]:
        return self._runner.run(self._async_client.get_devices(**kwargs))

    # ... etc
```

### 3.8 CLI

Typer + Rich with SilkCircuit colors, structured as subcommand groups:

```
hyper device list                    # List connected devices
hyper device show <id>               # Device details
hyper device discover                # Trigger discovery scan
hyper device identify <id>           # Flash device LEDs

hyper effect list                    # List all effects
hyper effect show <id>               # Effect details + controls
hyper effect apply <id> [--control key=val ...]  # Apply effect
hyper effect active                  # Show current effect
hyper effect stop                    # Stop active effect

hyper layout list                    # List layouts
hyper layout apply <id>              # Set active layout

hyper scene list                     # List scenes
hyper scene activate <id>            # Trigger scene

hyper profile list                   # List profiles
hyper profile apply <id>             # Apply profile

hyper preset list                    # List presets
hyper preset apply <id>              # Apply preset

hyper playlist list                  # List playlists
hyper playlist start <id>            # Start playlist
hyper playlist stop                  # Stop playlist

hyper status                         # System status overview
hyper health                         # Health check (exit code 0/1)
hyper monitor                        # Live WebSocket event stream
hyper spectrum                       # Live audio spectrum visualizer (sparklines)
```

**SilkCircuit formatting example:**
```python
PALETTE = {
    "electric_purple": "#e135ff",
    "neon_cyan": "#80ffea",
    "coral": "#ff6ac1",
    "electric_yellow": "#f1fa8c",
    "success_green": "#50fa7b",
    "error_red": "#ff6363",
}

def device_table(devices: list[DeviceInfo]) -> Table:
    table = Table(title="[bold #80ffea]Devices[/]", border_style="#e135ff")
    table.add_column("Name", style="#80ffea bold")
    table.add_column("Vendor", style="dim")
    table.add_column("LEDs", style="#ff6ac1", justify="right")
    table.add_column("Status", style="bold")
    for d in devices:
        status = "[#50fa7b]connected[/]" if d.state.connected else "[#ff6363]offline[/]"
        table.add_row(d.name, d.vendor, str(d.led_count), status)
    return table
```

---

## 4. hypercolor-homeassistant

### 4.1 Project Setup

```
hypercolor-homeassistant/
├── custom_components/
│   └── hypercolor/
│       ├── __init__.py          # Entry lifecycle (setup/unload)
│       ├── config_flow.py       # UI config flow + options flow
│       ├── const.py             # Domain, defaults, platform list
│       ├── coordinator.py       # DataUpdateCoordinator subclass
│       ├── entity.py            # Base entity mixin (DeviceInfo, state)
│       ├── light.py             # Light platform (brightness, effects)
│       ├── select.py            # Select entities (layout, profile, preset)
│       ├── button.py            # Button entities (discover, identify, scene triggers)
│       ├── sensor.py            # Sensor entities (FPS, LED count, audio level)
│       ├── switch.py            # Switch entities (per-device enable/disable)
│       ├── number.py            # Number entities (effect controls: ranges)
│       ├── manifest.json        # HA integration manifest
│       ├── strings.json         # UI strings
│       └── translations/
│           └── en.json
├── tests/
│   ├── conftest.py
│   ├── test_init.py
│   ├── test_config_flow.py
│   ├── test_coordinator.py
│   ├── test_light.py
│   ├── test_select.py
│   ├── test_button.py
│   ├── test_sensor.py
│   ├── test_switch.py
│   └── test_number.py
├── hacs.json
├── pyproject.toml
└── .github/workflows/ci-cd.yml
```

### 4.2 Key Differences From SignalRGB Integration

| Aspect | SignalRGB HA | Hypercolor HA |
|--------|-------------|---------------|
| Coordinator | Multiple ad-hoc coordinators | Single `HypercolorCoordinator` subclass + WS event push |
| Platforms | Light, Select, Button | Light, Select, Button, Sensor, Switch, Number |
| Devices | One device per entry | Multi-device — each physical device gets its own HA device |
| Update strategy | Polling (5 min) | WebSocket events + fallback polling |
| Auth | None | Optional API key in config flow |
| Controls | Static | Dynamic `number` entities from effect control schema |

### 4.3 Coordinator

The SignalRGB integration uses raw coordinators per-platform. Hypercolor uses a single smart coordinator backed by WebSocket events.

```python
class HypercolorCoordinator(DataUpdateCoordinator[HypercolorState]):
    """Central coordinator: WS events for instant updates, polling as fallback."""

    def __init__(self, hass: HomeAssistant, client: HypercolorClient, entry: ConfigEntry):
        super().__init__(
            hass,
            _LOGGER,
            name=DOMAIN,
            update_interval=timedelta(seconds=60),  # Fallback poll (events are primary)
            config_entry=entry,
        )
        self.client = client
        self._event_stream: HypercolorEventStream | None = None
        self._devices: dict[str, DeviceInfo] = {}

    async def _async_setup(self) -> None:
        """Connect WebSocket for real-time events."""
        self._event_stream = self.client.events()
        await self._event_stream.connect()
        await self._event_stream.subscribe("events")
        self._event_stream.on("effect_changed", self._on_effect_changed)
        self._event_stream.on("device_connected", self._on_device_changed)
        self._event_stream.on("device_disconnected", self._on_device_changed)
        self._event_stream.on("brightness_changed", self._on_brightness_changed)
        self._event_stream.on("profile_applied", self._on_profile_applied)
        self._event_stream.on("layout_changed", self._on_layout_changed)
        # Start listening in background task
        self.config_entry.async_create_background_task(
            hass, self._listen(), "hypercolor_ws_listener"
        )

    async def _async_update_data(self) -> HypercolorState:
        """Full state fetch (fallback polling + initial load)."""
        try:
            devices, effects, active, layouts, profiles, status = await asyncio.gather(
                self.client.get_devices(),
                self.client.get_effects(),
                self.client.get_active_effect(),
                self.client.get_layouts(),
                self.client.get_profiles(),
                self.client.get_status(),
            )
            return HypercolorState(
                devices={d.id: d for d in devices},
                effects=effects,
                active_effect=active,
                layouts=layouts,
                profiles=profiles,
                status=status,
            )
        except HypercolorConnectionError as err:
            raise UpdateFailed(f"Connection failed: {err}") from err

    async def _on_effect_changed(self, event: Event) -> None:
        """Push update from WebSocket — no polling needed."""
        active = await self.client.get_active_effect()
        self.data = replace(self.data, active_effect=active)
        self.async_set_updated_data(self.data)
```

### 4.4 Entity Platforms

#### Engine-Level Entities (one per Hypercolor instance)

**Light: `light.hypercolor_{host}`** — master engine control:
- On/off (enable/disable render engine)
- Brightness (0-255 HA → 0-100% daemon, controls global brightness)
- Effect selection (dropdown of all registered effects)
- Supported color modes: `BRIGHTNESS`
- Extra state attributes: active effect name, FPS, device count, audio active

**Select entities:**
- **Layout selector** `select.hypercolor_{host}_layout` — choose active spatial layout
- **Profile selector** `select.hypercolor_{host}_profile` — apply saved profile
- **Playlist selector** `select.hypercolor_{host}_playlist` — choose/stop playlist

**Button entities:**
- **Discover devices** `button.hypercolor_{host}_discover` — trigger device scan
- **Scene triggers** `button.hypercolor_{host}_scene_{name}` (per scene) — activate scene

**Sensor entities:**
- **FPS** `sensor.hypercolor_{host}_fps` — actual render framerate
- **Connected devices** `sensor.hypercolor_{host}_device_count` — count
- **Total LEDs** `sensor.hypercolor_{host}_led_count` — aggregate
- **Audio level** `sensor.hypercolor_{host}_audio_level` — if spectrum subscribed

**Number entities** (dynamic, per active effect control):
- Spawned from `ControlDefinition` schema when effect changes
- Range controls → `NumberEntity` with min/max/step
- `number.hypercolor_{host}_control_{control_id}`
- Removed when effect changes (dynamic entity management)

#### Per-Device Entities (one set per physical RGB device)

This is the #1 community request from the SignalRGB integration (issue #6) — users want
individual device control for static colors, adaptive lighting sync, and per-device brightness.
SignalRGB's API couldn't support this. Hypercolor's daemon exposes full per-device state.

**Light: `light.hypercolor_{device_name}`** — individual device control:
- On/off (enable/disable device output via `PUT /devices/{id}`)
- Brightness (per-device brightness, independent of global)
- Color modes: `RGB`, `COLOR_TEMP`, `BRIGHTNESS`
  - RGB mode enables static color on individual devices — the key unlock
  - Allows HA adaptive lighting to drive device colors directly
  - Color temp mode for warm/cool white devices that support it
- Effect attribute: shows which global effect is currently driving this device
- Extra state attributes:
  - `vendor`, `product`, `family` (device metadata)
  - `led_count`, `zone_count`
  - `connected` (bool)
  - `zones` (list of zone names)

**Switch: `switch.hypercolor_{device_name}_enabled`** — enable/disable device output:
- Simpler alternative to light on/off for automation use cases
- Toggling off removes device from the render loop without affecting other devices

**Button: `button.hypercolor_{device_name}_identify`** — flash device LEDs:
- Calls `POST /devices/{id}/identify`
- Useful for figuring out which physical device is which

**Sensor: `sensor.hypercolor_{device_name}_led_count`** — LED count for this device

#### Per-Device Color Control — The Key Feature

The per-device light entity is where Hypercolor leaps past SignalRGB's HA integration.
Three modes of operation:

**1. Effect-driven (default):**
Device receives colors from the global effect engine. Light entity shows current effect
name but color controls are read-only (reflecting what the engine is pushing).

**2. Static color override:**
User sets an RGB color or color temp via the HA light entity. This sends a per-device
color override to the daemon, pulling the device out of the global effect loop.
```python
# User sets light.hypercolor_keyboard to blue via HA
# → PATCH /devices/{id}/override { "color": [0, 100, 255] }
# Device now shows static blue while other devices continue running the effect
```

**3. Adaptive lighting sync:**
HA's adaptive lighting integration can drive per-device color temp, treating each
RGB device like a smart bulb. Combined with the static override API, this enables
circadian rhythm lighting on RGB peripherals — the exact use case users asked for in #6.

> **Note:** Static color override requires daemon support via a device override API
> endpoint. If the daemon doesn't have this yet, it becomes a daemon-side prerequisite.
> The HA integration should expose the entities regardless and gracefully degrade
> (show effect-driven colors as read-only) until the override API lands.

#### Entity ID Hygiene

Lesson from signalrgb-homeassistant issue #13: HA 2026.02 broke entity IDs with uppercase
chars. All entity IDs must be slugified — lowercase, underscores only, no special chars.

```python
from homeassistant.util import slugify

def _device_entity_id(device: DeviceInfo) -> str:
    """Generate clean entity ID from device name."""
    return slugify(f"hypercolor_{device.name}")
```

### 4.5 Multi-Device Model

Hypercolor manages multiple physical devices. The HA device registry gets a two-tier
hierarchy: one "hub" device for the daemon, with each physical RGB device as a child.

```python
# Top-level "daemon" device (hub)
DeviceInfo(
    identifiers={(DOMAIN, entry.entry_id)},
    name=f"Hypercolor ({host})",
    manufacturer="Hypercolor",
    model="Daemon",
    sw_version=status.version,
)

# Per physical device (linked via `via_device`)
# Each gets its own light, switch, button, and sensor entities
DeviceInfo(
    identifiers={(DOMAIN, f"{entry.entry_id}_{slugify(device.id)}")},
    name=device.name,
    manufacturer=device.vendor,
    model=device.product,
    via_device=(DOMAIN, entry.entry_id),
)
```

**Device discovery flow:**
1. Coordinator fetches `GET /devices` on startup and on `device_connected`/`device_disconnected` WS events
2. New devices → HA's `async_forward_entry_setups` creates entities for each platform
3. Removed devices → entities marked unavailable (not deleted, in case device reconnects)
4. Each platform's `async_setup_entry` iterates `coordinator.data.devices` and creates per-device entities

**Example HA device page for a Hypercolor instance:**
```
Hypercolor (hyperia.home)          ← hub device
├── Razer BlackWidow V4 Pro        ← child device
│   ├── light.hypercolor_blackwidow_v4_pro      (RGB color, brightness)
│   ├── switch.hypercolor_blackwidow_v4_pro_enabled
│   ├── button.hypercolor_blackwidow_v4_pro_identify
│   └── sensor.hypercolor_blackwidow_v4_pro_led_count
├── Corsair Lighting Node Pro      ← child device
│   ├── light.hypercolor_lighting_node_pro
│   ├── switch.hypercolor_lighting_node_pro_enabled
│   ├── button.hypercolor_lighting_node_pro_identify
│   └── sensor.hypercolor_lighting_node_pro_led_count
├── Razer Mouse Dock Chroma        ← child device
│   └── ...
└── (engine-level entities)
    ├── light.hypercolor_hyperia    (master brightness + effect select)
    ├── select.hypercolor_hyperia_layout
    ├── select.hypercolor_hyperia_profile
    ├── sensor.hypercolor_hyperia_fps
    └── ...
```

### 4.6 Config Flow

```python
class HypercolorConfigFlow(ConfigFlow, domain=DOMAIN):
    VERSION = 1

    async def async_step_user(self, user_input=None):
        errors = {}
        if user_input is not None:
            try:
                client = HypercolorClient(
                    host=user_input[CONF_HOST],
                    port=user_input[CONF_PORT],
                    api_key=user_input.get(CONF_API_KEY),
                )
                healthy = await client.health()
                if not healthy:
                    errors["base"] = "cannot_connect"
                else:
                    await self.async_set_unique_id(f"{user_input[CONF_HOST]}:{user_input[CONF_PORT]}")
                    self._abort_if_unique_id_configured()
                    return self.async_create_entry(
                        title=f"Hypercolor ({user_input[CONF_HOST]})",
                        data=user_input,
                    )
            except HypercolorConnectionError:
                errors["base"] = "cannot_connect"
            except HypercolorAuthenticationError:
                errors["base"] = "invalid_auth"
            except Exception:
                _LOGGER.exception("Unexpected error")
                errors["base"] = "unknown"

        return self.async_show_form(
            step_id="user",
            data_schema=vol.Schema({
                vol.Required(CONF_HOST): str,
                vol.Required(CONF_PORT, default=9420): int,
                vol.Optional(CONF_API_KEY): str,
            }),
            errors=errors,
        )

    @staticmethod
    @callback
    def async_get_options_flow(config_entry):
        return HypercolorOptionsFlow(config_entry)
```

**Options flow** — for post-setup tuning:
- Poll interval
- WebSocket enable/disable
- Audio spectrum subscription toggle
- Effect control entities toggle

---

## 5. hypercolor-card

### 5.1 Project Setup

```
hypercolor-card/
├── src/
│   ├── hypercolor-card.ts       # Main Lit component
│   ├── hypercolor-card-editor.ts # Config editor
│   ├── styles.css               # SilkCircuit-themed CSS
│   ├── state.ts                 # Reactive state controller
│   ├── state-manager.ts         # HA entity binding
│   ├── color-manager.ts         # ColorThief + Chroma.js theming
│   ├── config.ts                # Card config interface
│   ├── spectrum-viz.ts          # Audio spectrum visualization (canvas)
│   └── utils.ts                 # Color contrast, formatting
├── tests/
├── package.json
├── tsconfig.json
├── biome.json                   # Biome lint + format config
├── hacs.json
└── scripts/
```

**Build:** Pure Bun — no Vite, no Rollup, no Webpack. Single entry, single output:
```bash
bun build src/hypercolor-card.ts --outdir dist --minify --target browser
```

### 5.2 Upgrades Over hyper-light-card

| Aspect | hyper-light-card (SignalRGB) | hypercolor-card |
|--------|----------------------------|-----------------|
| Theme | Generic | SilkCircuit neon palette |
| Devices | Single entity | Multi-device tree view |
| Controls | None | Dynamic controls from effect schema |
| Audio | None | Spectrum visualizer (bass/mid/treble bars) |
| Scenes | None | Scene trigger buttons |
| Profiles | None | Profile quick-switch |
| Playlists | None | Playlist control (play/stop/skip) |
| Layouts | Dropdown | Visual layout preview |

### 5.3 Card Sections

```
┌─────────────────────────────────────┐
│  ⚡ Hypercolor          [toggle] │  Header + power
├─────────────────────────────────────┤
│  🎨 Aurora Borealis    ▼  ◀ 🎲 ▶  │  Effect selector + nav
├─────────────────────────────────────┤
│  ████████████████░░░░  72%         │  Brightness slider
├─────────────────────────────────────┤
│  Speed ████████░░  0.7             │  Dynamic controls
│  Color [■■■■]  #e135ff             │  (from effect schema)
│  Intensity ████░░░░  0.4           │
├─────────────────────────────────────┤
│  ▁▂▃▅▇█▇▅▃▂▁▂▃▅▇  🎵 spectrum     │  Audio visualizer
├─────────────────────────────────────┤
│  Devices: 5 connected  842 LEDs    │  Device summary
│  Layout: Full Room     Profile: ▼  │  Quick switches
└─────────────────────────────────────┘
```

### 5.4 SilkCircuit Theme Integration

CSS custom properties wired to SilkCircuit palette, with dynamic ColorThief extraction as override:

```css
:host {
  --hc-electric-purple: #e135ff;
  --hc-neon-cyan: #80ffea;
  --hc-coral: #ff6ac1;
  --hc-electric-yellow: #f1fa8c;
  --hc-success-green: #50fa7b;
  --hc-error-red: #ff6363;

  --hc-bg: var(--extracted-bg, #1a1a2e);
  --hc-accent: var(--extracted-accent, var(--hc-electric-purple));
  --hc-text: var(--extracted-text, #f0f0f0);
}
```

---

## 6. Tooling & Quality

### Python Projects (hypercolor-python + hypercolor-homeassistant)

| Gate | Tool | Config |
|------|------|--------|
| Format | `ruff format` | line-length=99 |
| Lint | `ruff check` | E, W, F, I, C, B, N, UP, RUF, ASYNC, S, TRY, PL |
| Types | `ty check` | strict mode |
| Test | `pytest` | asyncio_mode="auto", cov target 90%+ |
| Build | `hatchling` | src layout, PEP 660 |
| Env | `uv` | lockfile, Python 3.13 |

### TypeScript Project (hypercolor-card)

| Gate | Tool |
|------|------|
| Runtime / Build / Test | Bun (bundler + test runner) |
| Lint | Biome (replaces ESLint + Prettier — single fast tool) |
| Types | TypeScript strict |

---

## 7. CI/CD

### hypercolor-python

```yaml
# .github/workflows/ci-cd.yml
jobs:
  quality:
    steps:
      - uses: astral-sh/setup-uv@v6
      - run: uv sync --frozen
      - run: uv run ruff check .
      - run: uv run ruff format --check .
      - run: uv run ty check
      - run: uv run pytest --cov

  release:
    if: startsWith(github.ref, 'refs/tags/v')
    needs: quality
    steps:
      - run: uv build
      - uses: pypa/gh-action-pypi-publish@release/v1
```

### hypercolor-homeassistant

```yaml
jobs:
  quality:
    steps:
      - uses: astral-sh/setup-uv@v6
      - run: uv sync --frozen
      - run: uv run ruff check .
      - run: uv run ruff format --check .
      - run: uv run ty check
      - run: uv run pytest --cov
  validate:
    steps:
      - uses: home-assistant/actions/hassfest@master
      - uses: hacs/action@main
  release:
    if: startsWith(github.ref, 'refs/tags/v')
    needs: [quality, validate]
    steps:
      - # Create GitHub release with zip of custom_components/hypercolor/
```

### hypercolor-card

```yaml
jobs:
  build:
    steps:
      - uses: oven-sh/setup-bun@v2
      - run: bun install --frozen-lockfile
      - run: bun run check          # biome lint + format check
      - run: bun test
      - run: bun run build           # bun build (bundler)
  release:
    if: startsWith(github.ref, 'refs/tags/v')
    needs: build
    steps:
      - # Create GitHub release with hypercolor-card.js
```

---

## 8. Implementation Plan

### Phase 1: hypercolor-python (foundation)

Build order matters — the HA integration depends on the client.

| Step | Task | Est. |
|------|------|------|
| 1.1 | Scaffold project (`uv init`, pyproject.toml, ruff/ty config) | small |
| 1.2 | Models — all msgspec structs matching daemon API types | medium |
| 1.3 | Exceptions — full hierarchy | small |
| 1.4 | Async client — all REST endpoints | large |
| 1.5 | WebSocket client — event stream + binary frame parsing | large |
| 1.6 | Sync client wrapper | small |
| 1.7 | Tests — models, client (respx mocking), WebSocket | large |
| 1.8 | CLI — Typer commands with SilkCircuit formatting | medium |
| 1.9 | CI/CD pipeline | small |
| 1.10 | Docs (MkDocs Material) | medium |

### Phase 2: hypercolor-homeassistant

| Step | Task | Est. |
|------|------|------|
| 2.1 | Scaffold integration (manifest, const, strings) | small |
| 2.2 | Config flow (host/port/api_key) + options flow | medium |
| 2.3 | Coordinator — WS-backed with polling fallback | medium |
| 2.4 | Light platform | medium |
| 2.5 | Select platform (layout, profile, playlist) | medium |
| 2.6 | Button platform (discover, identify, scenes) | small |
| 2.7 | Sensor platform (FPS, devices, LEDs) | small |
| 2.8 | Switch platform (per-device enable) | small |
| 2.9 | Number platform (dynamic effect controls) | large |
| 2.10 | Tests — all platforms + coordinator | large |
| 2.11 | HACS validation + CI | small |

### Phase 3: hypercolor-card

| Step | Task | Est. |
|------|------|------|
| 3.1 | Scaffold (Vite + Lit + TypeScript) | small |
| 3.2 | State management + HA entity binding | medium |
| 3.3 | Card layout + SilkCircuit styling | medium |
| 3.4 | Effect selector + controls rendering | medium |
| 3.5 | Audio spectrum visualizer | medium |
| 3.6 | Device tree + layout preview | medium |
| 3.7 | Card editor (config UI) | medium |
| 3.8 | HACS setup + CI | small |

---

## 9. Open Questions

1. **Package name on PyPI:** `hypercolor` (simple) vs `hypercolor-python` (explicit)? Leaning `hypercolor`.

2. **msgspec vs pydantic vs mashumaro:** Spec assumes msgspec for speed + binary support. Pydantic v2 is also fast but heavier. mashumaro was used in signalrgb-python but msgspec is the 2026 choice.

3. **WebSocket in HA:** Should the integration maintain a persistent WS connection for instant state updates, or stick with polling? Spec assumes WS + polling fallback. WS is better UX but adds complexity.

4. **Dynamic number entities:** When the active effect changes, its control schema changes. Should we create/destroy `number` entities dynamically, or use a fixed set of generic slots? Dynamic is cleaner but may confuse HA automations that reference entity IDs.

5. **CLI name:** `hyper` (conflicts with Hypercolor's existing Rust CLI `hyper`)? Alternative: `hcl`, `hypercolor`, `hcolor`.

6. **Spectrum visualizer in card:** Canvas-based waveform or CSS bar chart? Canvas is smoother but heavier. CSS bars are simpler and may be enough.

7. **monorepo vs polyrepo:** Three separate repos (like SignalRGB stack) or a monorepo? Separate repos match the existing pattern and allow independent versioning.

---

## 10. Recommendation

**Go polyrepo, build in order, ship incrementally.**

Three separate repos under `~/dev/`:
- `hypercolor-python` — publish to PyPI as `hypercolor`
- `hypercolor-homeassistant` — HACS default repository
- `hypercolor-card` — HACS Lovelace plugin

**Start with `hypercolor-python`** — it's the foundation. Get the async client + models right, then the HA integration and card are mostly wiring.

**Use msgspec** — it's the right tool for 2026. Zero-copy binary decode for WebSocket frames, fast JSON for REST, built-in validation. mashumaro served signalrgb-python well but msgspec is strictly better for this use case.

**WebSocket-first HA integration** — unlike SignalRGB (polling-only), Hypercolor has a rich event system. Use it. The coordinator pattern with WS events + polling fallback gives the best UX with the reliability HA expects.

**SilkCircuit everywhere** — the CLI, the card, even the HA device icons. This is your design system; lean into it.

**Python 3.13 minimum** — it's 2026. Use `asyncio.Runner`, PEP 695 generics (`type X = ...`), and all the good stuff.
