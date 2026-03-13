+++
title = "CLI Reference"
description = "Command reference for the hyper CLI tool"
weight = 3
template = "page.html"
+++

The `hyper` command-line tool controls the Hypercolor daemon over its REST API. It supports styled table output, plain text, and JSON for scripting.

## Global Options

```
hyper [OPTIONS] <COMMAND>

Options:
  --format <FORMAT>    Output format: table, json, plain [default: table]
  --host <HOST>        Daemon host [default: localhost]
  --port <PORT>        Daemon port [default: 9420]
  --api-key <KEY>      API key (or set HYPERCOLOR_API_KEY env var)
  -j, --json           JSON output (shorthand for --format json)
  -q, --quiet          Suppress non-essential output
  --no-color           Disable colored output
  -v, --verbose        Increase verbosity (-v, -vv, -vvv)
```

## Commands

### `hyper status`

Show the current system state — running effect, connected devices, audio status, FPS.

```bash
hyper status
```

```
Effect:  Borealis (borealis)
FPS:     60.0
Devices: 3 connected
Audio:   enabled (level: 0.42)
```

### `hyper devices`

Device discovery and management.

```bash
hyper devices list              # List all devices
hyper devices discover          # Trigger device discovery
hyper devices identify <id>     # Flash a device for identification
```

Example output:

```
ID                          Name                    Backend   LEDs   Status
razer-blackwidow-v4-001     Razer BlackWidow V4     razer     126    connected
wled-living-room            WLED Living Room        wled      300    connected
prism8-001                  Lian Li Prism 8         prismrgb  1008   connected
```

### `hyper effects`

Browse and control effects.

```bash
hyper effects list              # List all available effects
hyper effects apply <id>        # Apply an effect
hyper effects stop              # Stop the current effect
hyper effects active            # Show the active effect and its controls
hyper effects rescan            # Rescan the effects directory
```

Apply with custom controls:

```bash
hyper effects apply borealis --control speed=7 --control palette=SilkCircuit
```

### `hyper scenes`

Scene management for automated lighting triggers.

```bash
hyper scenes list               # List all scenes
hyper scenes create <name>      # Create a scene from current state
hyper scenes activate <id>      # Activate a scene
hyper scenes delete <id>        # Delete a scene
```

### `hyper profiles`

Save and restore complete lighting states.

```bash
hyper profiles list             # List saved profiles
hyper profiles create <name>    # Save current state as a profile
hyper profiles apply <id>       # Apply a saved profile
hyper profiles delete <id>      # Delete a profile
```

### `hyper library`

Manage the effect library — favorites, presets, playlists.

```bash
hyper library favorites         # List favorites
hyper library presets           # List presets
hyper library playlists         # List playlists
```

### `hyper layouts`

Spatial layout management.

```bash
hyper layouts list              # List layouts
hyper layouts active            # Show the active layout
hyper layouts apply <id>        # Apply a layout
```

### `hyper config`

Configuration management.

```bash
hyper config show               # Show full configuration
hyper config get <key>          # Get a specific value
hyper config set <key> <value>  # Set a value
```

### `hyper service`

Daemon service lifecycle management.

```bash
hyper service status            # Check daemon status
```

### `hyper diagnose`

Run system diagnostics.

```bash
hyper diagnose
```

Checks device connectivity, audio capture status, effect engine health, USB permissions, and configuration validity. Outputs a diagnostic report with pass/fail status for each check.

## Output Formats

The `--format` flag controls how results are rendered:

- **`table`** (default) — Styled, aligned tables with color
- **`json`** — Machine-readable JSON for scripting and piping
- **`plain`** — Minimal text output without formatting

```bash
# Pipe device list to jq for filtering
hyper devices list -j | jq '.[] | select(.status == "connected")'

# Use in shell scripts
EFFECT_COUNT=$(hyper effects list -j | jq length)
echo "Found $EFFECT_COUNT effects"
```

## Environment Variables

| Variable | Description |
|---|---|
| `HYPERCOLOR_API_KEY` | API key for authenticated requests |
| `HYPERCOLOR_HOST` | Daemon host (overrides default `localhost`) |
| `HYPERCOLOR_PORT` | Daemon port (overrides default `9420`) |
