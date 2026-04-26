+++
title = "CLI Reference"
description = "Command reference for the hypercolor CLI tool"
weight = 3
template = "page.html"
+++

The `hypercolor` command-line tool controls the Hypercolor daemon over its REST API. It supports styled table output, plain text, and JSON for scripting.

## Global Options

```
hypercolor [OPTIONS] <COMMAND>

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

### `hypercolor status`

Show the current system state — running effect, connected devices, audio status, FPS.

```bash
hypercolor status
```

```
Effect:  Borealis (borealis)
FPS:     60.0
Devices: 3 connected
Audio:   enabled (level: 0.42)
```

### `hypercolor devices`

Device discovery and management.

```bash
hypercolor devices list              # List all devices
hypercolor devices discover          # Trigger device discovery
hypercolor devices identify <id>     # Flash a device for identification
```

Example output:

```
ID                          Name                    Backend   LEDs   Status
razer-blackwidow-v4-001     Razer BlackWidow V4     razer     126    connected
wled-living-room            WLED Living Room        wled      300    connected
prism8-001                  Lian Li Prism 8         prismrgb  1008   connected
```

### `hypercolor effects`

Browse and control effects.

```bash
hypercolor effects list              # List all available effects
hypercolor effects activate <id>     # Activate an effect
hypercolor effects stop              # Stop the current effect
hypercolor effects info              # Show the active effect and its controls
```

Apply with custom controls:

```bash
hypercolor effects activate borealis --param speed=7 --param palette=SilkCircuit
```

### `hypercolor scenes`

Scene management for automated lighting triggers.

```bash
hypercolor scenes list               # List all scenes
hypercolor scenes create <name>      # Create a scene from current state
hypercolor scenes activate <id>      # Activate a scene
hypercolor scenes delete <id>        # Delete a scene
```

### `hypercolor profiles`

Save and restore complete lighting states.

```bash
hypercolor profiles list             # List saved profiles
hypercolor profiles create <name>    # Save current state as a profile
hypercolor profiles apply <id>       # Apply a saved profile
hypercolor profiles delete <id>      # Delete a profile
```

### `hypercolor library`

Manage the effect library — favorites, presets, playlists.

```bash
hypercolor library favorites         # List favorites
hypercolor library presets           # List presets
hypercolor library playlists         # List playlists
```

### `hypercolor layouts`

Spatial layout management.

```bash
hypercolor layouts list              # List all layouts
hypercolor layouts show <id>         # Show layout details
hypercolor layouts update <id>       # Update a layout
```

### `hypercolor config`

Configuration management.

```bash
hypercolor config show               # Show full configuration
hypercolor config get <key>          # Get a specific value
hypercolor config set <key> <value>  # Set a value
```

### `hypercolor service`

Daemon service lifecycle management.

```bash
hypercolor service status            # Check daemon status
```

### `hypercolor diagnose`

Run system diagnostics.

```bash
hypercolor diagnose
```

Checks device connectivity, audio capture status, effect engine health, USB permissions, and configuration validity. Outputs a diagnostic report with pass/fail status for each check.

### `hypercolor servers`

Discover Hypercolor daemons on the local network.

```bash
hypercolor servers discover          # Find daemons via mDNS
```

### `hypercolor tui`

Launch the interactive terminal dashboard. Auto-starts a local daemon if one isn't already running.

```bash
hypercolor tui
```

### `hypercolor completions`

Generate shell completions.

```bash
hypercolor completions bash          # Bash completions
hypercolor completions zsh           # Zsh completions
hypercolor completions fish          # Fish completions
```

## Output Formats

The `--format` flag controls how results are rendered:

- **`table`** (default) — Styled, aligned tables with color
- **`json`** — Machine-readable JSON for scripting and piping
- **`plain`** — Minimal text output without formatting

```bash
# Pipe device list to jq for filtering
hypercolor devices list -j | jq '.[] | select(.status == "connected")'

# Use in shell scripts
EFFECT_COUNT=$(hypercolor effects list -j | jq length)
echo "Found $EFFECT_COUNT effects"
```

## Environment Variables

| Variable             | Description                        |
| -------------------- | ---------------------------------- |
| `HYPERCOLOR_API_KEY` | API key for authenticated requests |
