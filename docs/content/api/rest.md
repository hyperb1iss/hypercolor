+++
title = "REST API"
description = "HTTP API reference for the Hypercolor daemon"
weight = 1
template = "page.html"
+++

The Hypercolor daemon serves a REST API on port **9420** (configurable). All endpoints are under the `/api/v1` prefix. Success responses use a consistent JSON envelope:

```json
{
  "data": {},
  "meta": {
    "api_version": "1.0",
    "request_id": "req_019b1f9a-3f4b-7c8d-a2e1-91b4c0d86a25",
    "timestamp": "2026-05-13T05:12:00Z"
  }
}
```

Use `meta.request_id` when reporting API errors or correlating logs.

When an API key is configured, include it as a Bearer token:

```
Authorization: Bearer <your-api-key>
```

## System

{% api_endpoint(method="GET", path="/health") %}
Health check. Returns `200 OK` when the daemon is running. No authentication required.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/status") %}
Current system status including running effect, connected devices, audio state, and performance metrics.

**Response:**

```json
{
  "data": {
    "running": true,
    "version": "0.1.0",
    "device_count": 3,
    "effect_count": 32,
    "active_effect": "borealis",
    "global_brightness": 85,
    "audio_available": true,
    "render_loop": {
      "state": "running",
      "target_fps": 60,
      "actual_fps": 59.8
    }
  },
  "meta": {
    "api_version": "1.0",
    "request_id": "req_019b1f9a-3f4b-7c8d-a2e1-91b4c0d86a25",
    "timestamp": "2026-05-13T05:12:00Z"
  }
}
```

{% end %}

{% api_endpoint(method="GET", path="/api/v1/server") %}
Server identity and version information.
{% end %}

## Effects

{% api_endpoint(method="GET", path="/api/v1/effects") %}
List all available effects. Returns `data.items`, an array of effect summaries with ID, name, description, tags, and whether the effect is audio-reactive.

**Response:**

```json
{
  "data": {
    "items": [
      {
        "id": "borealis",
        "name": "Borealis",
        "description": "Aurora borealis with domain-warped fBm noise",
        "author": "Hypercolor",
        "category": "ambient",
        "source": "html",
        "runnable": true,
        "tags": ["ambient", "shader"],
        "version": "1.0.0",
        "audio_reactive": false
      }
    ],
    "pagination": {
      "offset": 0,
      "limit": 50,
      "total": 32,
      "has_more": false
    }
  },
  "meta": {
    "api_version": "1.0",
    "request_id": "req_019b1f9a-3f4b-7c8d-a2e1-91b4c0d86a25",
    "timestamp": "2026-05-13T05:12:00Z"
  }
}
```

{% end %}

{% api_endpoint(method="GET", path="/api/v1/effects/{id}") %}
Get detailed information about a specific effect, including its full control definitions with types, ranges, and defaults.

**Response:**

```json
{
  "data": {
    "id": "borealis",
    "name": "Borealis",
    "description": "Aurora borealis with domain-warped fBm noise",
    "author": "Hypercolor",
    "category": "ambient",
    "source": "html",
    "runnable": true,
    "tags": ["ambient", "shader"],
    "version": "1.0.0",
    "audio_reactive": false,
    "controls": []
  },
  "meta": {
    "api_version": "1.0",
    "request_id": "req_019b1f9a-3f4b-7c8d-a2e1-91b4c0d86a25",
    "timestamp": "2026-05-13T05:12:00Z"
  }
}
```

{% end %}

{% api_endpoint(method="POST", path="/api/v1/effects/{id}/apply") %}
Apply an effect to the current output. Optionally include control values to override defaults.

**Request body (optional):**

```json
{
  "controls": {
    "speed": 7,
    "palette": "SilkCircuit"
  }
}
```

**Response:**

```json
{
  "data": {
    "effect": {
      "id": "borealis",
      "name": "Borealis"
    },
    "applied_controls": {
      "speed": 7,
      "palette": "SilkCircuit"
    },
    "layout": null,
    "transition": {
      "type": "none",
      "duration_ms": 0
    },
    "warnings": []
  },
  "meta": {
    "api_version": "1.0",
    "request_id": "req_019b1f9a-3f4b-7c8d-a2e1-91b4c0d86a25",
    "timestamp": "2026-05-13T05:12:00Z"
  }
}
```

{% end %}

{% api_endpoint(method="GET", path="/api/v1/effects/active") %}
Get the currently active effect and its control values.
{% end %}

{% api_endpoint(method="PATCH", path="/api/v1/effects/current/controls") %}
Update control values on the currently running effect. Changes apply on the next frame.

**Request body:**

```json
{
  "controls": {
    "speed": 3,
    "intensity": 90
  }
}
```

{% end %}

{% api_endpoint(method="POST", path="/api/v1/effects/current/reset") %}
Reset all controls on the currently running effect to their default values.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/effects/stop") %}
Stop the currently running effect. LEDs go dark.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/effects/rescan") %}
Trigger a rescan of the effects directory. Use this after building new effects to pick them up without restarting the daemon.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/effects/install") %}
Install an effect from an uploaded file. Use this to deploy a freshly built HTML effect bundle to the daemon's effect library without a manual file copy.

**Request body:** Multipart form upload with the effect file.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/effects/{id}/cover") %}
Get the cover image for a specific effect.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/effects/active/cover") %}
Get the cover image for the currently active effect.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/effects/{id}/layout") %}
Get the layout associated with a specific effect.
{% end %}

{% api_endpoint(method="PUT", path="/api/v1/effects/{id}/layout") %}
Associate a specific effect with a layout.
{% end %}

{% api_endpoint(method="DELETE", path="/api/v1/effects/{id}/layout") %}
Clear the layout association for a specific effect.
{% end %}

{% api_endpoint(method="PATCH", path="/api/v1/effects/{id}/controls") %}
Update control values on a specific effect by ID (as opposed to the currently active effect).
{% end %}

{% api_endpoint(method="PUT", path="/api/v1/effects/current/controls/{name}/binding") %}
Bind a named control on the currently running effect to an input source (audio band, sensor, etc.).
{% end %}

## Devices

{% api_endpoint(method="GET", path="/api/v1/devices") %}
List all discovered and connected devices. Returns `data.items`.

**Response:**

```json
{
  "data": {
    "items": [
      {
        "id": "razer-blackwidow-v4-001",
        "layout_device_id": "razer-blackwidow-v4-001",
        "name": "Razer BlackWidow V4",
        "status": "connected",
        "brightness": 100,
        "total_leds": 126,
        "zones": []
      }
    ],
    "pagination": {
      "offset": 0,
      "limit": 50,
      "total": 1,
      "has_more": false
    }
  },
  "meta": {
    "api_version": "1.0",
    "request_id": "req_019b1f9a-3f4b-7c8d-a2e1-91b4c0d86a25",
    "timestamp": "2026-05-13T05:12:00Z"
  }
}
```

{% end %}

{% api_endpoint(method="GET", path="/api/v1/devices/{id}") %}
Get detailed information about a specific device including zones, LED layout, firmware version, and attachment configuration.
{% end %}

{% api_endpoint(method="PUT", path="/api/v1/devices/{id}") %}
Update device settings (name, zone assignments, brightness).
{% end %}

{% api_endpoint(method="DELETE", path="/api/v1/devices/{id}") %}
Remove a device from tracking.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/devices/discover") %}
Trigger device discovery across all backends. Returns newly found devices.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/devices/{id}/identify") %}
Flash a device's LEDs to help identify it physically.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/devices/{id}/zones/{zone_id}/identify") %}
Flash a specific zone on a device to identify it.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/devices/{id}/controls") %}
Get the control surface for a specific device — fields, types, and current values.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/devices/{id}/attachments") %}
Get the attachment configuration for a device.
{% end %}

{% api_endpoint(method="PUT", path="/api/v1/devices/{id}/attachments") %}
Update the attachment configuration for a device.
{% end %}

{% api_endpoint(method="DELETE", path="/api/v1/devices/{id}/attachments") %}
Clear attachment configuration from a device.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/devices/{id}/attachments/preview") %}
Preview attachment placement on a device without persisting the change.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/devices/{id}/attachments/{slot_id}/identify") %}
Identify a specific attachment slot on a device by flashing its LEDs.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/devices/{id}/logical-devices") %}
List logical device segments defined for this physical device.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/devices/{id}/logical-devices") %}
Create a new logical device segment on a physical device.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/devices/{id}/pair") %}
Initiate pairing for a device that requires authentication.
{% end %}

{% api_endpoint(method="DELETE", path="/api/v1/devices/{id}/pair") %}
Remove the pairing for a device.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/devices/metrics") %}
Get a per-device output telemetry snapshot (frame counts, errors, latency).
{% end %}

## Logical Devices

{% api_endpoint(method="GET", path="/api/v1/logical-devices") %}
List all logical device segments across all physical devices.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/logical-devices/{id}") %}
Get a specific logical device segment.
{% end %}

{% api_endpoint(method="PUT", path="/api/v1/logical-devices/{id}") %}
Update a logical device segment.
{% end %}

{% api_endpoint(method="DELETE", path="/api/v1/logical-devices/{id}") %}
Delete a logical device segment.
{% end %}

## Drivers

{% api_endpoint(method="GET", path="/api/v1/drivers") %}
List all registered driver modules with their ID, name, and connection state.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/drivers/{id}/config") %}
Get the configuration for a specific driver module.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/drivers/{id}/controls") %}
Get the control surface for a specific driver module — fields, types, and current values.
{% end %}

## Displays

Display devices are physical screens that can show HTML effects via the display-face system.

{% api_endpoint(method="GET", path="/api/v1/displays") %}
List all connected display devices.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/displays/{id}/preview.jpg") %}
Get a JPEG preview frame from a display device.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/displays/{id}/face") %}
Get the active display-face effect configuration for a display device.
{% end %}

{% api_endpoint(method="PUT", path="/api/v1/displays/{id}/face") %}
Set the display-face effect on a display device. Associates an HTML effect with the device in the active scene.
{% end %}

{% api_endpoint(method="DELETE", path="/api/v1/displays/{id}/face") %}
Remove the display-face assignment from a display device.
{% end %}

{% api_endpoint(method="PATCH", path="/api/v1/displays/{id}/face/controls") %}
Update control values on the active display-face effect for a display device.
{% end %}

{% api_endpoint(method="PATCH", path="/api/v1/displays/{id}/face/composition") %}
Update composition parameters (blend mode, z-order, opacity) for a display-face render group.
{% end %}

## Simulators

Virtual display simulators let you develop and test display-face effects without physical hardware.

{% api_endpoint(method="GET", path="/api/v1/simulators/displays") %}
List all simulated display devices.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/simulators/displays") %}
Create a new simulated display device.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/simulators/displays/{id}") %}
Get a specific simulated display.
{% end %}

{% api_endpoint(method="PATCH", path="/api/v1/simulators/displays/{id}") %}
Update a simulated display's configuration.
{% end %}

{% api_endpoint(method="DELETE", path="/api/v1/simulators/displays/{id}") %}
Delete a simulated display.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/simulators/displays/{id}/frame") %}
Get the latest composited frame from a simulated display.
{% end %}

## Attachments

Attachment templates describe physical accessories (keycaps, case panels, stands) that clip onto device slots and have their own LED zones.

{% api_endpoint(method="GET", path="/api/v1/attachments/templates") %}
List all available attachment templates (built-in and user-defined).
{% end %}

{% api_endpoint(method="POST", path="/api/v1/attachments/templates") %}
Create a new user-defined attachment template.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/attachments/templates/{id}") %}
Get a specific attachment template.
{% end %}

{% api_endpoint(method="PUT", path="/api/v1/attachments/templates/{id}") %}
Update a user-defined attachment template.
{% end %}

{% api_endpoint(method="DELETE", path="/api/v1/attachments/templates/{id}") %}
Delete a user-defined attachment template.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/attachments/categories") %}
List all attachment categories (e.g., keycap-set, case-panel, stand).
{% end %}

{% api_endpoint(method="GET", path="/api/v1/attachments/vendors") %}
List all attachment vendors with available templates.
{% end %}

## Control Surfaces

Control surfaces expose typed fields and actions for dynamic device or driver configuration (e.g., WLED protocol selection, Hue bridge IP). The web UI reads these surfaces to render device-specific settings panels.

{% api_endpoint(method="GET", path="/api/v1/control-surfaces") %}
List all registered control surfaces across devices and drivers.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/control-surfaces/{surface_id}") %}
Get a specific control surface with current field values.
{% end %}

{% api_endpoint(method="PATCH", path="/api/v1/control-surfaces/{surface_id}/values") %}
Apply typed field values to a control surface.

**Request body:**

```json
{
  "fields": {
    "protocol": { "type": "enum", "value": "ddp" },
    "ip_address": { "type": "ip", "value": "10.0.0.50" }
  }
}
```

{% end %}

{% api_endpoint(method="POST", path="/api/v1/control-surfaces/{surface_id}/actions/{action_id}") %}
Invoke a typed control surface action (e.g., "Discover", "Sync", "Reset").
{% end %}

## Scenes

{% api_endpoint(method="GET", path="/api/v1/scenes") %}
List all defined scenes.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/scenes") %}
Create a new scene with effect, controls, and optional transition settings.

**Request body:**

```json
{
  "name": "Late Night",
  "effect_id": "ambient-glow",
  "controls": { "speed": 2, "brightness": 30 },
  "transition": { "duration_ms": 2000, "easing": "ease_in_out" }
}
```

{% end %}

{% api_endpoint(method="GET", path="/api/v1/scenes/{id}") %}
Get a specific scene's configuration.
{% end %}

{% api_endpoint(method="PUT", path="/api/v1/scenes/{id}") %}
Update an existing scene.
{% end %}

{% api_endpoint(method="DELETE", path="/api/v1/scenes/{id}") %}
Delete a scene.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/scenes/active") %}
Get the currently active scene and its configuration.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/scenes/{id}/activate") %}
Activate a scene, applying its effect and controls with the configured transition.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/scenes/deactivate") %}
Deactivate the current scene, returning to the default free-running state.
{% end %}

## Profiles

{% api_endpoint(method="GET", path="/api/v1/profiles") %}
List all saved profiles.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/profiles") %}
Create a new profile from the current state.

**Request body:**

```json
{
  "name": "Gaming"
}
```

{% end %}

{% api_endpoint(method="GET", path="/api/v1/profiles/{id}") %}
Get a specific profile's saved state.
{% end %}

{% api_endpoint(method="PUT", path="/api/v1/profiles/{id}") %}
Update a profile.
{% end %}

{% api_endpoint(method="DELETE", path="/api/v1/profiles/{id}") %}
Delete a profile.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/profiles/{id}/apply") %}
Apply a profile, restoring its saved effect, controls, and device assignments.
{% end %}

## Layouts

{% api_endpoint(method="GET", path="/api/v1/layouts") %}
List all spatial layouts.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/layouts") %}
Create a new spatial layout defining how the effect canvas maps to physical LED positions.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/layouts/active") %}
Get the currently active layout.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/layouts/{id}") %}
Get a specific layout's configuration including device zones, positions, and LED mappings.
{% end %}

{% api_endpoint(method="PUT", path="/api/v1/layouts/{id}") %}
Update a layout.
{% end %}

{% api_endpoint(method="DELETE", path="/api/v1/layouts/{id}") %}
Delete a layout.
{% end %}

{% api_endpoint(method="PUT", path="/api/v1/layouts/active/preview") %}
Preview a layout without applying it permanently. Returns the zone-to-LED mapping that would result, so the UI can render a visual preview.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/layouts/{id}/apply") %}
Apply a layout as the active spatial mapping.
{% end %}

## Library

### Favorites

{% api_endpoint(method="GET", path="/api/v1/library/favorites") %}
List favorited effects.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/library/favorites") %}
Add an effect to favorites.

**Request body:**

```json
{
  "effect_id": "borealis"
}
```

{% end %}

{% api_endpoint(method="DELETE", path="/api/v1/library/favorites/{effect}") %}
Remove an effect from favorites.
{% end %}

### Presets

{% api_endpoint(method="GET", path="/api/v1/library/presets") %}
List saved presets (effect + control value combinations).
{% end %}

{% api_endpoint(method="POST", path="/api/v1/library/presets") %}
Save the current effect and control values as a named preset.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/library/presets/{id}") %}
Get a specific preset.
{% end %}

{% api_endpoint(method="PUT", path="/api/v1/library/presets/{id}") %}
Update a preset.
{% end %}

{% api_endpoint(method="DELETE", path="/api/v1/library/presets/{id}") %}
Delete a preset.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/library/presets/{id}/apply") %}
Apply a preset, setting its effect and control values.
{% end %}

### Playlists

{% api_endpoint(method="GET", path="/api/v1/library/playlists") %}
List all playlists.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/library/playlists") %}
Create a new playlist of effects with transition timing.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/library/playlists/active") %}
Get the currently running playlist, if any.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/library/playlists/{id}") %}
Get a specific playlist.
{% end %}

{% api_endpoint(method="PUT", path="/api/v1/library/playlists/{id}") %}
Update a playlist.
{% end %}

{% api_endpoint(method="DELETE", path="/api/v1/library/playlists/{id}") %}
Delete a playlist.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/library/playlists/{id}/activate") %}
Start playing a playlist. Effects cycle according to the playlist's timing configuration.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/library/playlists/stop") %}
Stop the currently running playlist.
{% end %}

## Settings

{% api_endpoint(method="GET", path="/api/v1/settings/brightness") %}
Get the current global brightness level.
{% end %}

{% api_endpoint(method="PUT", path="/api/v1/settings/brightness") %}
Set the global brightness level (0.0 to 1.0).

**Request body:**

```json
{
  "brightness": 0.8
}
```

{% end %}

{% api_endpoint(method="GET", path="/api/v1/audio/devices") %}
List available audio capture devices for reactive effects.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/system/sensors") %}
Get the latest hardware sensor snapshot — CPU temperature, GPU load, RAM usage, and raw component readings.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/system/sensors/{label}") %}
Get a single named sensor reading. Common labels: `cpu_temp`, `gpu_load`, `ram_used`.
{% end %}

## Configuration

{% api_endpoint(method="GET", path="/api/v1/config") %}
Show the full current configuration.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/config/get?key=path.to.key") %}
Get a specific configuration value by dotted key path.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/config/set") %}
Set a configuration value.

**Request body:**

```json
{
  "key": "audio.enabled",
  "value": true
}
```

{% end %}

{% api_endpoint(method="POST", path="/api/v1/config/reset") %}
Reset a configuration value to its default.

**Request body:**

```json
{
  "key": "audio.device_name"
}
```

{% end %}

## Diagnostics

{% api_endpoint(method="POST", path="/api/v1/diagnose") %}
Run system diagnostics. Checks device connectivity, audio capture status, effect engine health, and configuration validity.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/diagnose/memory") %}
Get a memory diagnostics snapshot — daemon RSS, Servo renderer RSS, canvas buffer size, and allocation counters. Useful when diagnosing slow memory growth.
{% end %}
