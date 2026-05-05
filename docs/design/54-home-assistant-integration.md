# 54. RFC: Home Assistant Integration

**Status:** Draft, revised three times on 2026-05-05 after three codex architecture passes. First pass fixed auth validation, push-only coordinator topology, async-only client policy, hub-and-spoke device registry, well-known live-control Number entities only, full manifest spec, realistic phasing. Second pass tightened HA lifecycle: WebSocket cleanup with try/finally, reauth flow via `_get_reauth_entry` and `async_update_reload_and_abort`, device removal via `async_update_device(remove_config_entry_id=...)` plus `async_remove_config_entry_device`, entity-registry cleanup on per-device opt-out, services registered in `async_setup`, accurate Bronze quality-scale checklist, Number entities omit `state_class`, redaction set preserves correlation IDs. Third pass corrected reauth kwarg (`data_updates`, not `data`), added `docs-high-level-description` to Bronze checklist, replaced invalid `NumberDeviceClass.None` with omission, added `config_entry_id` service targeting for multi-instance setups, and added zeroconf rediscovery host/port updates with `_abort_if_unique_id_configured(updates=...)`.
**Date:** 2026-05-05.
**Author:** Bliss (with Nova).
**Depends on:** Hypercolor Python client (`python/`), daemon REST + WS API, mDNS publisher (`crates/hypercolor-daemon/src/mdns.rs`).

## Summary

Ship a Home Assistant integration that exposes Hypercolor as a first-class HA citizen. Users get a master light entity, native HA controls for every Hypercolor concept (effects, scenes, profiles, layouts, presets, library), and audio-reactive primitives that work as automation triggers across the whole home. The integration uses the Python client's async surface only, talks REST + WebSocket, discovers daemons via zeroconf, and registers each physical LED device as a child of the daemon hub in HA's device registry. Distributed as a custom HACS repo from day one.

The differentiator over `signalrgb-homeassistant` (the prior-art reference): WebSocket-driven push state instead of 5-minute polling, scene/profile/layout entities reflecting Hypercolor's richer composition model, per-effect presets, audio beat/energy as HA primitives for cross-system reactivity, and proper hub-and-spoke device topology so users see their actual LED devices in the HA Devices UI.

## Goals

1. **Zero-config setup on the same network.** Daemon advertises mDNS, HA picks it up, user clicks confirm.
2. **Every daemon capability reachable from HA.** Effects, scenes, profiles, layouts, library favorites/presets/playlists, displays, brightness, audio reactivity, identify-device, diagnostics.
3. **Real-time state.** No polling lag for state changes. WebSocket events drive coordinators directly.
4. **Audio reactivity as HA primitive.** Beat detection and energy as sensors, usable in automations across HA.
5. **Clean parity with the daemon's API.** Custom services mirror the user-facing operations the API exposes, minus anything an entity already covers.
6. **Per-device control is opt-in.** Default is a single master light, users with rich setups enable per-device entities through the options flow.
7. **Hub-and-spoke device registry.** Users see their daemon and their LED devices as proper HA Devices, not a flat entity list.

## Non-goals

- **Auth-protected setups in v1 are best-effort.** API key support exists in the config flow with proper validation, the integration is still tuned for localhost or trusted-LAN. TLS termination is out of scope.
- **MCP exposure to HA voice assistants.** The daemon already speaks MCP, routing that through HA assistants is future work.
- **Default-HACS submission in v1.** Custom repo distribution only. Submission to default HACS is a possible v2 task once the integration is field-tested.
- **Native HA scene capture/restore for Hypercolor state.** Hypercolor profiles already do this, HA scenes don't need to duplicate it.
- **Effect authoring UI in HA.** An `upload_effect` service exists, the SDK and AI Studio are the authoring paths.
- **Dynamic per-effect Number entities.** Rejected as an HA anti-pattern (see Live controls policy).

## Minimum supported Home Assistant version

**HA 2024.4.0 or newer.** This unlocks `entry.runtime_data` for clean per-entry state. `SupportsResponse` (added in 2023.7) is used for response-data services. Older HA versions are explicitly unsupported and will fail `hassfest` validation against the manifest.

## Repository layout

The integration lives in a new repo `~/dev/hypercolor-homeassistant`, structured per HACS conventions:

```
hypercolor-homeassistant/
  custom_components/hypercolor/
    __init__.py          # async_setup_entry, async_unload_entry, runtime_data wiring
    manifest.json        # Integration metadata, requirements, zeroconf
    config_flow.py       # Zeroconf + manual flow, reauth flow, options flow
    coordinator.py       # Push-only coordinators + reconcile task
    const.py             # Domain, defaults, channel names, redaction set
    runtime_data.py      # HypercolorRuntimeData dataclass
    light.py             # Master + opt-in per-device lights
    select.py            # Scene, profile, layout, preset, audio device
    sensor.py            # FPS, render time, audio energy, hardware sensors
    binary_sensor.py     # Connected, audio beat, audio reactive active
    button.py            # Discover, next/prev/random, identify, stop
    switch.py            # Audio reactive toggle, per-device enable
    number.py            # Static well-known live controls
    services.yaml        # Service schemas
    services.py          # Service handlers
    diagnostics.py       # async_get_config_entry_diagnostics + device diagnostics
    repairs.py           # Repair issue handlers (auth fail, unreachable, version mismatch)
    strings.json         # Config flow + service i18n
    translations/en.json
  hacs.json
  README.md
  examples/
    dashboards/audio_reactive_room.yaml
    automations/sunset_warm.yaml
    automations/bass_dim.yaml
  tests/
    test_config_flow.py
    test_coordinator.py
    test_light.py
    test_select.py
    test_services.py
    test_brightness.py
    test_reconcile.py
    test_reauth.py
  pyproject.toml
```

## Python library prerequisites

The Hypercolor Python client at `python/` is in solid shape: async via `httpx` and `websockets`, type-safe via `msgspec` and `attrs`, ruff/ty clean, 85% coverage minimum. Generated OpenAPI client wrapped by handcrafted public surface in `client.py`.

A `SyncHypercolorClient` wrapper exists at `python/src/hypercolor/sync_client.py` for blocking call sites outside HA. **The HA integration does not use it** (see Async-only policy below).

Six items to close before the integration starts. None are architectural.

| Gap | File | Effort |
|-----|------|--------|
| Wrap library/favorites in `HypercolorClient` | `python/src/hypercolor/client.py` | ~20 lines |
| Wrap scene CRUD beyond `activate` | `client.py` | ~40 lines |
| Wrap presets list/get/apply/save/delete | `client.py` | ~50 lines |
| Wrap playlists list/get/play | `client.py` | ~30 lines |
| WebSocket auto-reconnect with backoff and resubscribe | `python/src/hypercolor/websocket.py` | ~120 lines |
| HTTP retry on 5xx/timeout via tenacity | `client.py` | ~40 lines |

Total: 4 to 6 hours including tests. These land as a single PR to `python/` before the integration repo is created.

The `HypercolorClient` constructor must also accept an injected `httpx.AsyncClient` so HA can pass its shared pooled client (added if not already present, ~10 lines).

## Async-only client policy

The integration uses `HypercolorClient` (the async class) exclusively. `SyncHypercolorClient` calls `loop.run_until_complete` on a private event loop (see `python/src/hypercolor/sync_client.py:43`), which deadlocks inside HA's running loop and trips HA's blocking-call detector.

Concrete rules:

- **HTTP transport:** uses `homeassistant.helpers.httpx_client.get_async_client(hass)` so the integration shares HA's pooled `AsyncClient`. The Python library accepts an injected client through `HypercolorClient(httpx_client=...)`.
- **WebSocket lifecycle:** runs as `entry.async_create_background_task(hass, _run_websocket(...), name="hypercolor.ws")` so HA cancels it cleanly on unload.
- **Per-entry state:** lives in `entry.runtime_data` (a `HypercolorRuntimeData` dataclass holding the client, coordinators, WS task handle, reconcile task handle, and connection state). No `hass.data[DOMAIN][entry.entry_id]` indirection.
- **All I/O paths are awaited.** No `asyncio.run`, no `loop.run_until_complete`, no `hass.async_add_executor_job` wrappers around library calls.

A pytest fixture asserts that `SyncHypercolorClient` is never imported anywhere in `custom_components/hypercolor/` to catch regressions.

## Config flow and discovery

The daemon advertises `_hypercolor._tcp.local.` with TXT records for `id`, `name`, `version`, `api`, `auth`. The integration registers as a zeroconf consumer in `manifest.json`:

```json
"zeroconf": [{"type": "_hypercolor._tcp.local."}]
```

### Flow steps

1. **Zeroconf discovery.** `async_step_zeroconf` fires when HA sees the daemon. The flow extracts `instance_id` from the TXT record, calls `await self.async_set_unique_id(instance_id)` and `self._abort_if_unique_id_configured(updates={"host": discovery.host, "port": discovery.port})` so a re-discovery on a new IP refreshes the existing entry's host/port instead of being a no-op. If the entry is new, the flow routes to a confirmation step that shows the instance name, host, and version.
2. **Manual entry.** `async_step_user` collects host (default `127.0.0.1`), port (default `9420`), and optional API key. Same validation path as zeroconf after the user submits.
3. **Validation (two-step).**
   - First, `GET /api/v1/server` (auth-exempt, see `crates/hypercolor-daemon/src/api/security.rs:362`) to capture `instance_id` and the daemon's `auth_required` flag. This step verifies reachability.
   - **If `auth_required` is true**, the flow makes a second call to a control-tier endpoint (`GET /api/v1/effects`) with the bearer token. A 401 surfaces an `invalid_auth` error in the form. Without this second step, an invalid API key would silently pass setup because `/server` is intentionally exempt for discovery probes.
   - In the manual-step path, the flow also calls `_abort_if_unique_id_mismatch()` after `async_set_unique_id(instance_id)` so a user manually entering host/port for an instance that's already configured under a different host gets a clear "already configured" abort instead of a duplicate entry.
4. **Reauth.** When a stored API key starts returning 401 during normal operation, the integration raises `ConfigEntryAuthFailed` from the operation site, which triggers HA's reauth flow.
   - `async_step_reauth(entry_data)` captures the existing entry via `self._get_reauth_entry()` and routes to `async_step_reauth_confirm`.
   - `async_step_reauth_confirm` shows a form pre-filled from the existing entry's `host` and `port`, asks only for a new API key, and runs the same two-step validation helper used by user-step.
   - On success, the integration calls `self.async_update_reload_and_abort(entry, data_updates={"api_key": new_key})`, which updates the entry, reloads it, and dismisses the reauth notification atomically. The kwarg is `data_updates` (a partial patch), not `data` (full replacement).
   - The reauth helper is a single shared function so user-step, zeroconf-step, and reauth-step all converge on identical validation logic.

### Options flow

Versioned from day one. Initial schema:

```python
VERSION = 1
MINOR_VERSION = 1

OPTIONS_DEFAULTS = {
    "reconcile_interval_s": 60,
    "channels.audio": False,
    "channels.metrics": False,
    "channels.device_metrics": False,
    "per_device_entities": [],          # list of device_id strings
    "live_controls_enabled": True,      # toggles well-known Number entities
    "audio_beat_hold_ms": 100,
    "disconnect_grace_s": 5,
    "unavailable_after_s": 30,
}
```

`async_migrate_entry(hass, entry)` walks `entry.version` and `entry.minor_version` and applies migrations in order. Day-one shape avoids future scrambles when option keys are added or renamed. Bumping `MINOR_VERSION` allows additive changes (new key, new default); bumping `VERSION` requires a real migration.

### unique_id semantics

`unique_id = instance_id` (UUID7 from the daemon, persisted in the daemon's data dir per `crates/hypercolor-daemon/src/startup/config.rs:113-138`). Stable across daemon restarts and IP changes. Rejected alternatives: `host:port` (changes on network reshuffles), `mac_address` (not always available, server may have multiple NICs).

DHCP fallback discovery is deliberately not added. HA prefers zeroconf when the device advertises it, and the daemon does. DHCP discovery can be a v2 add if real users hit zeroconf-blocked networks.

## Update strategy

### Coordinator topology (push-only + separate reconciler)

The integration uses **push-driven coordinators** with a **separate reconciliation task**, not a polling `DataUpdateCoordinator` with `async_set_updated_data` overlaid on top. Mixing the two paths is a known footgun: HA's polling coordinator resets its `update_interval` timer every time `async_set_updated_data` is called, so a busy WebSocket would defeat the 60-second drift correction completely.

Five coordinators, each subclassing `DataUpdateCoordinator` with `update_interval=None` (push-only, no internal poll):

- `state_coordinator`: active effect, scene, profile, layout, brightness, FPS digest
- `catalog_coordinator`: effect catalog, scene list, profile list, layout list, presets per active effect
- `device_coordinator`: device list with topology and connection status
- `metrics_coordinator`: FPS, render time, render-stage breakdown (only when `channels.metrics` enabled)
- `audio_coordinator`: spectrum digest, beat detection, energy (only when `channels.audio` enabled)

A single `_reconcile_loop` background task (`entry.async_create_background_task`) runs every `reconcile_interval_s` (default 60) and force-refreshes state, catalog, and device coordinators via REST. The reconciler doesn't touch metrics or audio (those are pure WS streams; if disconnected, they're stale by definition until reconnect).

### WebSocket lifecycle

```
async_setup_entry
  ├── client = HypercolorClient(httpx_client=get_async_client(hass), ...)
  ├── coordinators = build_coordinators(hass, entry, client)
  ├── runtime_data = HypercolorRuntimeData(client, coordinators, ws_task=None, reconcile_task=None, connection_state=...)
  ├── initial REST fetch of state/catalog/devices to seed coordinators (await coordinators.state.async_refresh(), etc.)
  ├── register hub Device + child Devices (see Device registry topology)
  ├── ws_task = entry.async_create_background_task(hass, _run_websocket(...), name="hypercolor.ws")
  └── reconcile_task = entry.async_create_background_task(hass, _reconcile_loop(...), name="hypercolor.reconcile")

_run_websocket loop:
  while not hass.is_stopping:
    ws = None
    try:
      ws = await client.connect_websocket(subprotocols=["hypercolor.v1"])
      await ws.subscribe(channels=["events", *opted_in])
      hello = await ws.recv_hello()                 # daemon sends initial state on connect
      await _reconcile_from_hello(hello)            # full coordinator refresh from hello state + REST
      _set_connected(True)
      async for msg in ws:
        _dispatch(msg)                              # routes deltas to the right coordinator
    except asyncio.CancelledError:
      raise                                          # propagate so HA's task cancellation completes
    except (ConnectionClosed, OSError):
      pass                                           # falls through to backoff
    finally:
      _set_connected(False)
      if ws is not None:
        with contextlib.suppress(Exception):
          await ws.close()
    await asyncio.sleep(_backoff_with_jitter())
```

The `finally` block guarantees the WebSocket is closed when HA cancels the background task during config-entry unload, even if the cancellation hits while `recv` is awaiting. Without it, a leaked socket survives the unload and the next setup races against it.

### Reconnect reconciliation

When the WebSocket reconnects after any disconnect, the integration **must reconcile**, not just resubscribe:

1. Drop any in-flight delta merges from before the reconnect.
2. Consume the `hello.state` snapshot the daemon sends on connect.
3. Force a fresh REST pull of state, catalog, and device coordinators (through their reconciler entry points). This is the same path the 60-second reconciler uses.
4. Resubscribe to all opted-in channels.
5. Resume normal delta processing.

Without step 3, missed events during a long disconnect (effect added, device renamed, scene activated by another HA client or the TUI) leave coordinator data permanently stale until the next 60-second reconcile pass, which is unacceptable for a "real-time" integration.

### Disconnect grace

Transient TCP blips are normal. `binary_sensor.hypercolor_connected` doesn't flip to off until `disconnect_grace_s` (default 5) elapses. Entities don't go `STATE_UNAVAILABLE` until `unavailable_after_s` (default 30). Both tunable via options flow.

## Device registry topology

The integration models the daemon as a **hub** and each physical LED device as a **child** in HA's device registry. Users see a clean Devices UI grouped under the daemon, not a flat entity list.

```python
# Hub Device (one per config entry)
DeviceInfo(
    identifiers={(DOMAIN, instance_id)},
    name=instance_name,
    manufacturer="Hypercolor",
    model="Daemon",
    sw_version=daemon_version,
    configuration_url=f"http://{host}:{port}",
)

# Child Device (one per physical LED device)
DeviceInfo(
    identifiers={(DOMAIN, f"{instance_id}:{device_id}")},
    name=device.display_name,
    manufacturer=device.vendor,
    model=device.model,
    via_device=(DOMAIN, instance_id),  # links child to hub
    connections=device.connections,    # MAC for network devices, USB path for USB
)
```

### Entity attachment

- **System-wide entities** (master light, scene/profile/layout/preset selects, FPS sensor, audio entities, daemon-level buttons) attach to the **hub** device.
- **Per-device entities** (per-device light, identify button, enable switch) attach to the **child** device for that LED device.

### Lifecycle

When the daemon's device list changes (user removed a device, hardware unplugged), the integration reconciles HA's device registry by calling `device_registry.async_update_device(device_id, remove_config_entry_id=entry.entry_id)`. This is the HA-canonical removal path: it dissociates the Device from the config entry, and HA disposes of the Device when no entries remain on it. Calling `async_remove_device` directly is wrong because it can leave orphan registry rows.

The integration also implements `async_remove_config_entry_device(hass, entry, device)` so users can remove a stale Device from the HA UI manually. The hook returns `True` once the integration has confirmed the device is gone from the daemon (or the user wants it gone regardless of daemon state).

When a new device appears on the daemon, a child Device is registered. Per-device entities are created for that device only if the device's `device_id` is in `options["per_device_entities"]`.

When a user removes a device from `options["per_device_entities"]` via the options flow, the integration explicitly cleans up the entity registry: it iterates the unique IDs for that device's per-device entities (`light`, `button`, `switch`) and calls `entity_registry.async_remove(entity_id)` for each. Without this step, disabled-then-removed entities leak orphan registry rows that surface as ghost entities in HA's UI.

## Entity model

### Default entities (always created)

| Entity | Source | Attached to | Purpose |
|--------|--------|-------------|---------|
| `light.hypercolor` | active state + brightness | hub | Master on/off, brightness, current effect from catalog. See Light entity contract. |
| `select.hypercolor_scene` | scene list + active scene | hub | Switch active scene |
| `select.hypercolor_profile` | profile list | hub | Activate saved snapshots (effect + controls + brightness + layout) |
| `select.hypercolor_layout` | layout list | hub | Switch spatial layout |
| `select.hypercolor_preset` | per-effect presets | hub | Library + bundled presets, scoped to active effect, refreshes on effect change |
| `binary_sensor.hypercolor_connected` | WS connection state | hub | True when daemon reachable, debounced by `disconnect_grace_s` |
| `sensor.hypercolor_active_effect` | active state | hub | Effect name as text sensor for templates |
| `button.hypercolor_discover_devices` | service trigger | hub | Run device discovery scan |
| `button.hypercolor_next_effect` | catalog navigation | hub | Effect cycling |
| `button.hypercolor_previous_effect` | catalog navigation | hub | |
| `button.hypercolor_random_effect` | catalog navigation | hub | |
| `button.hypercolor_stop_effect` | service trigger | hub | Fade out current effect |

`select.hypercolor_effect` was removed. The native `light.effect` / `effect_list` mechanism on the master light is the canonical effect picker. A duplicate select entity creates two sources of truth that can drift.

### Opt-in entities (options flow toggles)

| Entity | Toggle | Attached to | Purpose |
|--------|--------|-------------|---------|
| `light.hypercolor_<device>` | `per_device_entities` | child | Per-device brightness override and on/off |
| `button.hypercolor_identify_<device>` | `per_device_entities` | child | Identify (LED pulse) for setup |
| `switch.hypercolor_<device>_enabled` | `per_device_entities` | child | Disable a device without removal |
| `number.hypercolor_brightness` | `live_controls_enabled` | hub | Well-known live control |
| `number.hypercolor_speed` | `live_controls_enabled` | hub | Well-known live control |
| `number.hypercolor_hue_shift` | `live_controls_enabled` | hub | Well-known live control |
| `number.hypercolor_intensity` | `live_controls_enabled` | hub | Well-known live control |
| `sensor.hypercolor_fps` | `channels.metrics` | hub | See Sensor metadata |
| `sensor.hypercolor_render_time_ms` | `channels.metrics` | hub | See Sensor metadata |
| `sensor.hypercolor_cpu_temp` | `channels.metrics` | hub | See Sensor metadata |
| `sensor.hypercolor_gpu_temp` | `channels.metrics` | hub | See Sensor metadata |
| `sensor.hypercolor_audio_energy` | `channels.audio` | hub | See Sensor metadata |
| `binary_sensor.hypercolor_audio_beat` | `channels.audio` | hub | Pulses on beat (configurable hold) |
| `binary_sensor.hypercolor_audio_reactive_active` | `channels.audio` | hub | True when active effect uses audio |
| `select.hypercolor_audio_device` | `channels.audio` | hub | Audio input source picker |
| `switch.hypercolor_audio_reactive` | `channels.audio` | hub | Toggle audio input |

### Live controls policy (well-known controls only)

The previous draft proposed dynamically registering a Number entity per control on whatever effect was active. That is an **HA anti-pattern** for two reasons:

- Reusing a key like `number.hypercolor_speed` for whatever-speed-means across effects breaks state graphs in Logbook and History because the unit and range silently change between effects.
- Generating per-effect-per-control unique IDs (`number.hypercolor_<effect_id>_<control_key>`) creates registry blowup, hundreds of orphan entities after browsing the catalog, and stale automation references on every effect rename.

**v1 policy:** expose only **four well-known controls** as static Number entities: `brightness`, `speed`, `hue_shift`, `intensity`. The integration matches the active effect's control schema by name (case-insensitive, daemon-side normalization). If the active effect doesn't expose a matching control, the corresponding Number entity goes `STATE_UNAVAILABLE` rather than disappearing. Range and step come from the active effect's schema; defaults are reasonable HA-friendly values used while no effect is active.

Arbitrary effect-specific controls remain reachable through the `hypercolor.set_control` service. v2 may explore a richer per-effect surface (e.g., a custom Lovelace card driven by daemon WebSocket), but it won't be HA Number entities.

### Sensor metadata

| Entity | device_class | state_class | unit | Notes |
|--------|--------------|-------------|------|-------|
| `sensor.hypercolor_active_effect` | none | none | none | Text sensor |
| `sensor.hypercolor_fps` | none | `measurement` | `fps` (custom unit string) | |
| `sensor.hypercolor_render_time_ms` | `duration` | `measurement` | `ms` | |
| `sensor.hypercolor_cpu_temp` | `temperature` | `measurement` | `°C` | Only when daemon reports CPU sensor |
| `sensor.hypercolor_gpu_temp` | `temperature` | `measurement` | `°C` | Only when daemon reports GPU sensor |
| `sensor.hypercolor_audio_energy` | none | `measurement` | none | 0.0 to 1.0 normalized |

Binary sensors:

| Entity | device_class | Notes |
|--------|--------------|-------|
| `binary_sensor.hypercolor_connected` | `connectivity` | |
| `binary_sensor.hypercolor_audio_beat` | `sound` | Auto-off after `audio_beat_hold_ms` |
| `binary_sensor.hypercolor_audio_reactive_active` | none | |

Number entities declare `mode=NumberMode.SLIDER`, native min/max/step from the active effect's control schema, and `native_unit_of_measurement` from the schema where defined. They do **not** declare `state_class`, which is a Sensor-only attribute that `NumberEntityDescription` does not accept. None of the four well-known controls map to a meaningful `NumberDeviceClass`, so `device_class` is omitted entirely (it defaults to `None`).

## Light entity contract

`light.hypercolor` declares:

```python
_attr_supported_color_modes = {ColorMode.BRIGHTNESS}
_attr_color_mode = ColorMode.BRIGHTNESS
_attr_supported_features = LightEntityFeature.EFFECT | LightEntityFeature.TRANSITION
# _attr_effect_list refreshes from catalog_coordinator
```

`turn_on(brightness=..., effect=..., transition=...)`:

- `brightness` (HA 0-255) is mapped through the brightness formula and dispatched via the daemon's brightness API.
- `effect` triggers `apply_effect(effect_id)` with no controls override (preserves whatever's already set on that effect).
- `transition` becomes the daemon's transition duration parameter on the apply call.
- A solid color is set through the `hypercolor.set_color` service, **not** via the light entity. Adding `ColorMode.RGB` to the master light is rejected for v1: it implies the light has a single solid color, which contradicts the entire effects/scenes paradigm and would confuse users.

`turn_off()` calls `stop_effect` with a default fade.

Per-device lights (`light.hypercolor_<device>`) declare the same color modes and features. Their `turn_on(brightness)` patches the daemon's per-device brightness override, `turn_off()` clears that override.

Effect attribute (`light.hypercolor.attributes.effect`) is the daemon's active effect ID. Additional state attributes: `effect_metadata` (author, audio_reactive, tags), `active_scene`, `active_profile`, `active_layout`.

## Brightness mapping

HA uses 0-255, daemon uses 0-100. The integration uses a deterministic round-trip-stable mapping:

```python
def ha_to_daemon(ha: int) -> int:
    if ha <= 0:
        return 0
    return max(1, (ha * 100 + 127) // 255)

def daemon_to_ha(d: int) -> int:
    if d <= 0:
        return 0
    return max(1, (d * 255 + 50) // 100)
```

Round-trip property: every daemon value reads back identically through HA. Formally, `ha_to_daemon(daemon_to_ha(d)) == d` for every `d` in `0..=100`. An arbitrary HA value may snap to a nearby HA value on first write because HA has 256 levels and the daemon has 101, but subsequent reads stabilize. The `max(1, ...)` clamp on both sides prevents a non-zero brightness from collapsing to zero (which would silently turn the lights off when the user only meant "very dim").

Verification table (selected points):

| HA in | Daemon | HA back | Drift on first round-trip | Stable after first round? |
|-------|--------|---------|---------------------------|---------------------------|
| 1 | 1 | 3 | +2 (snaps to 3) | yes (3 → 1 → 3) |
| 3 | 1 | 3 | 0 | yes |
| 64 | 25 | 64 | 0 | yes |
| 128 | 50 | 128 | 0 | yes |
| 192 | 75 | 191 | -1 | yes (191 → 75 → 191) |
| 255 | 100 | 255 | 0 | yes |

A pytest fixture sweeps every daemon value in 0-100 and asserts `ha_to_daemon(daemon_to_ha(d)) == d`. A second fixture sweeps every HA value in 0-255 and asserts that the second round-trip is a fixed point (`f(f(x)) == f(x)` where `f = daemon_to_ha . ha_to_daemon`).

## Custom services

Services are async-only handlers registered against the Python client. The previous draft listed 22; codex flagged this as bloat. Trimmed to **16** by dropping anything an entity already covers.

Service registration follows HA's `action-setup` quality rule: services register **once** in `async_setup(hass, config)` (not in `async_setup_entry`), independent of any specific config entry. Registering inside `async_setup_entry` causes services to vanish during config-entry reloads, which breaks any automation mid-flight.

Targeting: every service schema **requires** a `config_entry_id` field selected via `selector.ConfigEntrySelector(integration="hypercolor")`. HA's selector UI auto-populates this when only one Hypercolor entry is configured, so the single-daemon UX stays one-click while the YAML and dev-tools surface remain explicit and unambiguous. The handler resolves through `hass.config_entries.async_get_entry(entry_id)` and dispatches against that entry's `runtime_data.client`. Services that already operate on a specific noun (effect, scene, profile, layout, preset, device, display) keep their existing schema fields alongside `config_entry_id`. The same shape applies uniformly to `list_presets` and `run_diagnostics`. Required-not-defaulted matches HA's current service-action guidance: required targets should not be optional or auto-resolved.

All services additionally require `config_entry_id` (see Targeting). The "Additional schema" column lists service-specific fields beyond that.

| Service | Additional schema | Response | Notes |
|---------|-------------------|----------|-------|
| `hypercolor.apply_effect` | `effect_id`, `controls`?, `transition`?, `preset_id`? | none | Optionally with preset or live controls |
| `hypercolor.set_color` | `r`, `g`, `b` or `hex` | none | Solid color shortcut |
| `hypercolor.set_control` | `control_name`, `value` | none | Live patch on active effect, including non-well-known controls |
| `hypercolor.activate_scene` | `scene_id` | none | |
| `hypercolor.create_scene` | `name`, `mutation_mode`?, `priority`? | optional | Returns created scene id when called with `return_response: true` |
| `hypercolor.activate_profile` | `profile_id` | none | |
| `hypercolor.save_profile` | `name` | optional | Returns created profile id |
| `hypercolor.apply_layout` | `layout_id` | none | |
| `hypercolor.apply_preset` | `preset_id` | none | Daemon resolves library vs bundled |
| `hypercolor.save_preset` | `name`, `effect_id`? | optional | Returns created preset id |
| `hypercolor.delete_preset` | `preset_id` | none | Library presets only |
| `hypercolor.list_presets` | `effect_id`? | **required** | `SupportsResponse.ONLY` |
| `hypercolor.identify_device` | `device_id`, `duration_ms`? | none | |
| `hypercolor.set_display_face` | `display_id`, `effect_id`, `controls`? | none | Multi-display setups |
| `hypercolor.upload_effect` | `path`, `name` | optional | Path validated against `hass.config.allowed_external_dirs` |
| `hypercolor.run_diagnostics` | _(only `config_entry_id`)_ | **required** | `SupportsResponse.ONLY`, returns full diagnostic report |

Dropped from the previous draft because entities cover them:

- `hypercolor.set_brightness` is covered by `light.turn_on(brightness=...)` on `light.hypercolor`.
- `hypercolor.stop_effect` is covered by `button.press` on `button.hypercolor_stop_effect`.
- `hypercolor.add_to_favorites` and `remove_from_favorites` are deferred to v2 along with full library/playlist surfacing (favorites aren't core to the master-light experience and don't yet have a clean entity mapping).
- `hypercolor.create_playlist` and `play_playlist` are deferred to v2.

`SupportsResponse.ONLY` is used for `list_presets` and `run_diagnostics` since their entire value is the response data. `SupportsResponse.OPTIONAL` is used for create-style services that return an ID. Other services use `SupportsResponse.NONE`.

Each service has a proper `services.yaml` entry with target selectors where applicable, validation, and i18n strings.

## Audio reactivity hook

Audio entities are the unlock `signalrgb-homeassistant` never had. Beat and energy as HA primitives means cross-system reactivity is one automation away:

```yaml
- alias: Bass dim main lights
  trigger:
    platform: state
    entity_id: binary_sensor.hypercolor_audio_beat
    to: "on"
  condition:
    - condition: numeric_state
      entity_id: sensor.hypercolor_audio_energy
      above: 0.7
  action:
    service: light.turn_on
    target:
      entity_id: light.living_room_main
    data:
      brightness_step_pct: -20
      transition: 0.1
```

Worth leading with this in the README. Real differentiator for users running media setups, parties, or studio rigs.

## Diagnostics

Use HA's first-party diagnostics helpers, not a custom file dump:

```python
async def async_get_config_entry_diagnostics(
    hass: HomeAssistant, entry: ConfigEntry
) -> dict[str, Any]:
    runtime: HypercolorRuntimeData = entry.runtime_data
    raw = await runtime.client.diagnose()
    return async_redact_data(
        {
            "config": {**entry.data, **entry.options},
            "daemon": raw,
            "ws_state": runtime.connection_state.snapshot(),
            "coordinator_summary": {
                name: c.last_update_success
                for name, c in runtime.coordinators.items()
            },
        },
        TO_REDACT,
    )

TO_REDACT = {
    "api_key", "api_keys", "password", "token", "bearer",
    "host", "external_host", "ip", "ip_address", "url",
    "mac", "mac_address",
}
```

`async_get_device_diagnostics` is also implemented for per-device dumps that include only that device's slice of the daemon report.

`instance_id` and `device_id` are deliberately **not** redacted. They're stable identifiers used for correlation across diagnostic dumps, log lines, and bug reports. Redacting them would force users to manually de-anonymize when sharing diagnostics, and they're not secrets (any LAN client that can reach the daemon can enumerate them). If a future privacy concern emerges, the fix is to hash these IDs (`sha256(instance_id)[:8]`) rather than blanket-redact, preserving correlation while eliminating the raw value.

The diagnostic blob is structurally the same shape `hypercolor.run_diagnostics` returns, so user-shared diagnostics from the HA UI and from the service action are interchangeable.

## Repair issues

Repairs are **only** for things the user can fix from HA. The previous draft proposed creating repair issues "for stale effect IDs after daemon catalog changes," which is too broad. Automations refer to effect IDs by name in YAML, and the catalog churns enough that this would spam the Repairs panel. Stale catalog references surface as service-call failures in the user's automation log, where they belong.

Concrete repair scenarios for v1, all registered via `async_create_issue`:

| Issue key | Trigger | User action | Severity |
|-----------|---------|-------------|----------|
| `auth_invalid` | Stored API key returns 401 during normal operation | "Re-authenticate" button triggers `async_step_reauth` | error |
| `daemon_unreachable` | Daemon unreachable for more than 10 minutes after grace | Show daemon URL, link to troubleshooting docs | warning |
| `daemon_too_old` | Daemon advertises version below integration's minimum | Link to daemon update path | error |

Each repair issue carries a `learn_more_url`, a `translation_key`, and optional `data` for the resolution flow. Resolution clears the issue via `async_delete_issue` once the underlying condition flips (next successful auth, next reconnect, next compatible daemon version).

## Manifest specification

`custom_components/hypercolor/manifest.json`:

```json
{
  "domain": "hypercolor",
  "name": "Hypercolor",
  "version": "0.1.0",
  "documentation": "https://github.com/hyperb1iss/hypercolor-homeassistant/blob/main/README.md",
  "issue_tracker": "https://github.com/hyperb1iss/hypercolor-homeassistant/issues",
  "codeowners": ["@hyperb1iss"],
  "config_flow": true,
  "iot_class": "local_push",
  "integration_type": "hub",
  "quality_scale": "bronze",
  "requirements": ["hypercolor>=0.1.0,<0.2.0"],
  "zeroconf": [{"type": "_hypercolor._tcp.local."}],
  "after_dependencies": ["zeroconf"]
}
```

`hacs.json`:

```json
{
  "name": "Hypercolor",
  "country": "US",
  "homeassistant": "2024.4.0",
  "render_readme": true
}
```

### Quality scale target

**Bronze** for v1. The HA quality scale ladder is strict about which rules belong where, so the v1 plan covers exactly the Bronze rules and stages the rest. Bronze rules covered by this RFC:

- `action-setup`: services registered in `async_setup` (see Custom services).
- `appropriate-polling`: push-only coordinators with separate reconciler explicitly avoid spurious polling (see Update strategy).
- `brands`: integration submits brand assets to home-assistant/brands before publishing.
- `common-modules`: `runtime_data.py`, `coordinator.py` host shared types and helpers.
- `config-flow`: zeroconf + manual + options.
- `config-flow-test-coverage`: pytest in `tests/test_config_flow.py`.
- `dependency-transparency`: `requirements` lists the Python client with a fixed version range.
- `docs-actions` / `docs-high-level-description` / `docs-installation-instructions` / `docs-removal-instructions`: README sections covering each rule.
- `entity-event-setup`: WebSocket dispatch wires through coordinators only, no entity-level subscriptions.
- `entity-unique-id`: every entity declares a stable unique ID derived from `instance_id` (and `device_id` for child entities).
- `has-entity-name`: entities use `_attr_has_entity_name = True`.
- `runtime-data`: `entry.runtime_data` is the only state container.
- `test-before-configure` / `test-before-setup`: config flow validates connectivity, `async_setup_entry` raises `ConfigEntryNotReady` when the daemon is unreachable.
- `unique-config-entry`: enforced via `async_set_unique_id(instance_id)` + `_abort_if_unique_id_configured()`.

Bronze rules **not** in scope for v1 (deferred to Silver in v2): config entry unloading explicit completeness audit, parallel-updates declarations, reauthentication flow polish, integration owner reachability commitments, log levels, exception handling refinements.

Bronze rules **not** in scope (deferred to Gold): devices and per-device diagnostics polish to Gold standards, dynamic-devices automatic add/remove flow polish, entity-translations completeness for non-English locales, exception strings translated, repair-flows audit, stale-devices polish.

Reauth, diagnostics, and repair issues are still implemented in v1 because they're table stakes for an integration with auth, they just don't all qualify for Bronze certification on their own. The v1 quality scale claim is **Bronze**, not "Bronze plus selected Silver and Gold rules."

`version` in `manifest.json` is the integration version (semver), not the Python library version. HACS uses both: manifest version for the integration release, GitHub release tag must match the manifest version.

## Phased delivery

Realistic estimates after the codex pass.

**Phase 1 (MVP, 2–3 weeks):**
- Repository scaffolding with CI (ruff, mypy/pyright, pytest, hassfest, HACS validator)
- Full `manifest.json` and `hacs.json`
- `async_setup_entry`, `async_unload_entry`, `entry.runtime_data`, `async_migrate_entry` skeleton
- Async-only `HypercolorClient` wired via `httpx_client.get_async_client(hass)`
- Five push coordinators + reconcile loop
- WebSocket background task with hello-state seeding and reconnect reconciliation
- Hub Device registered, child Devices for each LED device with `via_device` linkage
- Master light (`ColorMode.BRIGHTNESS` + effect/effect_list), scene/profile/layout/preset selects, connected binary sensor (debounced), active-effect text sensor
- Buttons: discover, next/prev/random, stop
- Zeroconf + manual config flow with two-step auth validation
- Reauth flow
- Brightness round-trip mapping with property test
- Core services (8): `apply_effect`, `apply_preset`, `save_preset`, `activate_scene`, `activate_profile`, `apply_layout`, `set_color`, `set_control`
- Diagnostics (config-entry + per-device) with redaction
- Three repair issues (auth fail, unreachable, version mismatch)
- English strings

**Phase 2 (1 week):**
- Per-device lights, identify buttons, per-device enable switches (attached to child Devices)
- Well-known Number entities (brightness/speed/hue_shift/intensity) gated by `live_controls_enabled`
- Options flow surface (channel toggles, per-device opt-in, live-controls toggle, reconcile interval, audio beat hold, disconnect grace)
- Remaining services: `create_scene`, `save_profile`, `delete_preset`, `list_presets`, `identify_device`, `set_display_face`, `upload_effect`, `run_diagnostics`

**Phase 3 (1 week):**
- Audio entities (beat with auto-off, energy, audio device select, audio reactive switch)
- Metrics sensors (FPS, render time, render-stage breakdown, CPU/GPU temp) with proper device_class/state_class/unit
- Multi-daemon namespacing (entity prefix from instance friendly name when more than one config entry exists)

**Phase 4 (publish, 1–2 weeks):**
- HACS custom-repo metadata, README with screenshots and the audio-reactive hook front-and-center
- `examples/` with Lovelace dashboards and audio-reactive automations
- Translations for de, fr, es, ja
- First public release tag

**Total realistic v1: 5–7 weeks** (codex's 4–6 week estimate plus a buffer for first-time HA-integration friction). HACS-polished release at ~6 weeks with light iteration.

## Locked decisions

1. **Distribution.** Custom HACS repo. No default-HACS submission attempt in v1.
2. **Per-device entities.** Opt-in via options flow, attached to child Devices.
3. **No MCP-to-HA bridge.**
4. **Number entities.** Static well-known controls (brightness/speed/hue_shift/intensity) only. Go unavailable when active effect lacks them. Dynamic per-effect Number entities rejected as an HA anti-pattern.
5. **`unique_id`.** Daemon `instance_id`, not `host:port`.
6. **Update strategy.** Push-only coordinators with separate reconcile task. Not a polling coordinator with `async_set_updated_data` overlay.
7. **Coordinator topology.** One per data domain (state, catalog, device, metrics, audio).
8. **Presets.** Single Select entity unifying library presets and bundled templates, scoped to active effect.
9. **Async-only client.** Integration uses `HypercolorClient` async surface only. `SyncHypercolorClient` is banned inside HA.
10. **Device registry.** Daemon as hub Device, each LED device as child with `via_device=(DOMAIN, instance_id)` linkage.
11. **Auth validation.** Two-step config flow validation: `/server` for discovery, control-tier endpoint with bearer for key verification.
12. **Master light color mode.** `ColorMode.BRIGHTNESS` only. Solid color via `hypercolor.set_color` service, not via the light entity.
13. **Brightness mapping.** Specified formula in this RFC. Daemon-side round-trip is exact for all values in `0..=100`; HA-side may snap by up to ±2 on first write but stabilizes thereafter.
14. **Service surface.** 16 services. Two use `SupportsResponse.ONLY` (`list_presets`, `run_diagnostics`). Drop services that have entity equivalents.
15. **HA minimum version.** 2024.4.0.
16. **Quality scale target.** Bronze for v1.
17. **Diagnostics.** First-party HA diagnostics helper with explicit redaction set.
18. **Repairs.** Limited to user-actionable scenarios (auth fail, unreachable daemon, version mismatch). Stale automation references are not repair issues.
19. **Discovery.** Zeroconf only. No DHCP fallback in v1.
20. **Options versioning.** `VERSION = 1`, `MINOR_VERSION = 1` from day one with `async_migrate_entry` scaffold.
21. **WebSocket cleanup.** `try/finally` around `recv` guarantees socket closure on cancellation. `asyncio.CancelledError` is re-raised, all other exceptions fall through to backoff.
22. **Service registration.** Services register once in `async_setup(hass, config)`, not in `async_setup_entry`. Handlers resolve target entries dynamically.
23. **Device removal.** Use `async_update_device(remove_config_entry_id=...)` and implement `async_remove_config_entry_device`. Never call `async_remove_device` directly.
24. **Entity registry cleanup on opt-out.** Removing a device from `per_device_entities` triggers explicit `entity_registry.async_remove(entity_id)` for that device's per-device entities.
25. **Reauth flow.** Uses `_get_reauth_entry()`, prefills host/port, validates through the shared two-step helper, completes via `async_update_reload_and_abort`.
26. **Number entities.** Omit `state_class` (Sensor-only attribute). Declare mode, min/max/step, unit, optional device_class.
27. **Redaction set.** Redacts secrets and network identifiers (api_key, host, IP, MAC). Preserves `instance_id` and `device_id` for cross-dump correlation.
28. **Service targeting.** Every service **requires** a `config_entry_id` field via `ConfigEntrySelector(integration="hypercolor")`. HA's selector UI auto-populates the value when a single daemon is configured, but the field is never optional or defaulted in code or `services.yaml`.
29. **Zeroconf rediscovery.** `_abort_if_unique_id_configured(updates={"host": ..., "port": ...})` keeps the existing entry's host/port fresh when the daemon's IP changes. Manual flow uses `_abort_if_unique_id_mismatch()` to catch duplicate entries.

## Open questions

1. **Multi-daemon namespacing.** When a user has two Hypercolor instances configured, do entity IDs auto-prefix with the instance friendly name, or does the user pick a prefix during config flow? Lean toward friendly-name-as-prefix during flow; less magic, more user control.
2. **Effect upload `path` semantics.** Does `hypercolor.upload_effect` accept any HA-allowlisted dir, or only `/config/hypercolor_effects/`? Lean toward only `/config/hypercolor_effects/` to keep the surface small and predictable.
3. **Audio beat hold default.** 100 ms feels right but should be field-tested against the "dim on bass" example automation. May need to be 50 ms for snappier reactivity or 200 ms to avoid double-triggers on percussive transients.
4. **Translations beyond English.** Phase 4 lists de/fr/es/ja but Hypercolor's likely audience may want Brazilian Portuguese (gaming/media) or Korean (RGB enthusiast scene). Confirm before phase 4.
5. **Per-device entity options-flow UX.** Showing a checkbox list of all known devices is fine for 2-5 devices, awful for 30+. Consider a "create per-device entities for new devices automatically" toggle with a per-device override list for excluding specific devices.
6. **Favorites and playlists in v1?** Currently deferred to v2 because they don't have clean entity mappings. If field-testing shows users want them, lift back into Phase 2 as services (`add_to_favorites`, `play_playlist`).

## References

### Hypercolor source

- Async client: `python/src/hypercolor/client.py`
- WebSocket client: `python/src/hypercolor/websocket.py`
- Sync wrapper (banned in HA): `python/src/hypercolor/sync_client.py:43`
- Daemon REST and WS API: `crates/hypercolor-daemon/src/api/`
- Auth-exempt path list: `crates/hypercolor-daemon/src/api/security.rs:362`
- Server identity persistence: `crates/hypercolor-daemon/src/startup/config.rs:113-138`
- WebSocket protocol: `crates/hypercolor-daemon/src/api/ws/protocol.rs`
- mDNS publisher: `crates/hypercolor-daemon/src/mdns.rs`
- API design RFC: `docs/design/05-api-design.md`

### Prior art

- `signalrgb-homeassistant` (reference implementation): `~/dev/signalrgb-homeassistant`

### Home Assistant docs

- Creating an integration manifest: https://developers.home-assistant.io/docs/creating_integration_manifest
- Config flow handler: https://developers.home-assistant.io/docs/config_entries_config_flow_handler
- DataUpdateCoordinator: https://developers.home-assistant.io/docs/integration_fetching_data
- Light entity: https://developers.home-assistant.io/docs/core/entity/light
- Sensor entity: https://developers.home-assistant.io/docs/core/entity/sensor
- Device registry: https://developers.home-assistant.io/docs/device_registry_index
- Entity registry: https://developers.home-assistant.io/docs/entity_registry_index
- Async + blocking I/O: https://developers.home-assistant.io/docs/asyncio_blocking_operations
- Service responses: https://developers.home-assistant.io/docs/dev_101_services
- Diagnostics: https://developers.home-assistant.io/docs/core/integration/diagnostics
- Repairs: https://developers.home-assistant.io/docs/core/integration/repairs
- Quality scale: https://developers.home-assistant.io/docs/core/integration-quality-scale

### HACS

- HACS publishing: https://hacs.xyz/docs/publish/start
- HACS integration requirements: https://hacs.xyz/docs/publish/integration
