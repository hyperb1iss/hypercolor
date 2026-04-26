+++
title = "REST API"
description = "HTTP API reference for the Hypercolor daemon"
weight = 1
template = "page.html"
+++

The Hypercolor daemon serves a REST API on port **9420** (configurable). All endpoints are under the `/api/v1` prefix. Responses are JSON.

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
  "running": true,
  "effect": { "id": "borealis", "name": "Borealis" },
  "devices": 3,
  "fps": 60.0,
  "audio": { "enabled": true, "level": 0.42 }
}
```

{% end %}

{% api_endpoint(method="GET", path="/api/v1/server") %}
Server identity and version information.
{% end %}

## Effects

{% api_endpoint(method="GET", path="/api/v1/effects") %}
List all available effects. Returns an array of effect summaries with ID, name, description, tags, and whether the effect is audio-reactive.

**Response:**

```json
[
  {
    "id": "borealis",
    "name": "Borealis",
    "description": "Aurora borealis with domain-warped fBm noise",
    "tags": ["ambient", "shader"],
    "audio_reactive": false
  }
]
```

{% end %}

{% api_endpoint(method="GET", path="/api/v1/effects/{id}") %}
Get detailed information about a specific effect, including its full control definitions with types, ranges, and defaults.

**Response:**

```json
{
  "id": "borealis",
  "name": "Borealis",
  "description": "Aurora borealis with domain-warped fBm noise",
  "controls": [
    {
      "id": "speed",
      "label": "Speed",
      "type": "number",
      "min": 1,
      "max": 10,
      "default": 5
    }
  ]
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
  "applied": true,
  "effect_id": "borealis"
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
  "speed": 3,
  "intensity": 90
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

## Devices

{% api_endpoint(method="GET", path="/api/v1/devices") %}
List all discovered and connected devices.

**Response:**

```json
[
  {
    "id": "razer-blackwidow-v4-001",
    "name": "Razer BlackWidow V4",
    "backend": "razer",
    "led_count": 126,
    "status": "connected"
  }
]
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

{% api_endpoint(method="POST", path="/api/v1/scenes/{id}/activate") %}
Activate a scene, applying its effect and controls with the configured transition.
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
