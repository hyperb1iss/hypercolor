+++
title = "Configuration"
description = "Configure profiles, audio input, device mappings, and daemon settings"
weight = 3
template = "page.html"
+++

## Config File Location

Hypercolor stores its configuration in TOML format:

```
~/.config/hypercolor/hypercolor.toml
```

The daemon creates a default config on first run. You can also view the current configuration via the API:

```bash
curl http://localhost:9420/api/v1/config | jq
```

## Key Settings

### Daemon

```toml
[daemon]
bind = "127.0.0.1:9420"    # Listen address and port
log_level = "info"          # trace, debug, info, warn, error

[mcp]
enabled = true              # Enable MCP server for AI integration
base_path = "/mcp"          # MCP endpoint path
```

### Audio Input

Hypercolor uses your system's audio capture for reactive effects. Configure which device to use:

```toml
[audio]
enabled = true
# Omit device_name to use the system default capture device.
# List available devices: curl localhost:9420/api/v1/audio/devices
device_name = "Monitor of Built-in Audio"
```

**Via REST API:**

```bash
# List available audio capture devices
curl http://localhost:9420/api/v1/audio/devices | jq
```

### Authentication

When the `HYPERCOLOR_API_KEY` environment variable is set, all API requests must include it:

```bash
curl -H "Authorization: Bearer <your-key>" http://localhost:9420/api/v1/status
```

The CLI can pass it via `--api-key` or the same `HYPERCOLOR_API_KEY` environment variable.

## Profile System

Profiles save your entire lighting state (active effect, control values, device assignments, brightness, and spatial layout) so you can switch between configurations instantly.

### Creating a Profile

Save the current state as a named profile:

```bash
# Via CLI
hyper profiles create "Gaming"

# Via REST API
curl -X POST http://localhost:9420/api/v1/profiles \
  -H "Content-Type: application/json" \
  -d '{"name": "Gaming"}'
```

### Applying a Profile

```bash
# Via CLI
hyper profiles apply "Gaming"

# Via REST API — use the profile ID from the list
curl -X POST http://localhost:9420/api/v1/profiles/<id>/apply
```

### Managing Profiles

```bash
# List all profiles
hyper profiles list

# Delete a profile
hyper profiles delete <id>
```

## Scenes

Scenes build on profiles by adding **triggers**: conditions that automatically activate a lighting state. A scene might activate a calmer effect when it's late at night, or switch to an audio-reactive mode when music starts playing.

```bash
# List scenes
curl http://localhost:9420/api/v1/scenes | jq

# Create a scene
curl -X POST http://localhost:9420/api/v1/scenes \
  -H "Content-Type: application/json" \
  -d '{
    "name": "Late Night",
    "effect_id": "ambient-glow",
    "controls": {"speed": 2, "brightness": 30},
    "transition": {"duration_ms": 2000, "easing": "ease_in_out"}
  }'

# Activate a scene manually
curl -X POST http://localhost:9420/api/v1/scenes/<id>/activate
```

## Spatial Layouts

Layouts define how the effect canvas maps to physical LED positions. Each device zone has a position, size, and rotation expressed in normalized [0.0, 1.0] coordinates, so layouts stay valid regardless of the configured canvas resolution (640x480 by default).

```bash
# List layouts
curl http://localhost:9420/api/v1/layouts | jq

# Get the currently active layout
curl http://localhost:9420/api/v1/layouts/active | jq
```

Layouts can also be managed through the web UI's visual layout editor, which provides drag-and-drop positioning of device zones.

## Configuration via API

You can read and write individual config values without editing the TOML file:

```bash
# Get a config value
curl "http://localhost:9420/api/v1/config/get?key=audio.enabled" | jq

# Set a config value
curl -X POST http://localhost:9420/api/v1/config/set \
  -H "Content-Type: application/json" \
  -d '{"key": "audio.enabled", "value": true}'

# Reset a value to its default
curl -X POST http://localhost:9420/api/v1/config/reset \
  -H "Content-Type: application/json" \
  -d '{"key": "audio.device_name"}'
```

{% callout(type="tip", title="Live reload") %}
Most configuration changes take effect immediately without restarting the daemon. The config manager watches the TOML file for changes and publishes update events through the event bus.
{% end %}
