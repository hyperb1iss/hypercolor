# 12 -- Configuration System Technical Specification

> Every file, every field, every default. The complete schema for Hypercolor's persistent state.

**Status:** Implementation-ready
**Crate:** `hypercolor-config`
**Module path:** `hypercolor_config::{daemon, profile, scene, layout, device, rules, loader, migrate}`

---

## Table of Contents

1. [Directory Layout](#1-directory-layout)
2. [Main Daemon Configuration](#2-main-daemon-configuration)
3. [Profile Format](#3-profile-format)
4. [Scene Format](#4-scene-format)
5. [Layout Format](#5-layout-format)
6. [Device Configuration](#6-device-configuration)
7. [Automation Rules](#7-automation-rules)
8. [Rust Types](#8-rust-types)
9. [Default Values](#9-default-values)
10. [Schema Versioning & Migration](#10-schema-versioning--migration)
11. [Environment Variable Overrides](#11-environment-variable-overrides)
12. [Cross-Platform Paths](#12-cross-platform-paths)

---

## 1. Directory Layout

Hypercolor follows the XDG Base Directory Specification on Linux and uses platform-standard paths on Windows. The directory structure is identical on both platforms; only the root prefix changes.

### 1.1 Linux (XDG)

```
$XDG_CONFIG_HOME/hypercolor/         # Default: ~/.config/hypercolor/
+-- hypercolor.toml                  # Main daemon config
+-- hypercolor.local.toml            # Machine-specific overrides (not in git)
+-- devices/                         # Per-device config + calibration
|   +-- hid-16d5-1f01-abc123.toml
|   +-- wled-living-room.toml
|   +-- razer-huntsman-v2.toml
+-- layouts/                         # Spatial layout definitions
|   +-- default.toml
|   +-- desktop-v2.toml
+-- profiles/                        # Saved lighting states
|   +-- gaming.toml
|   +-- chill.toml
|   +-- stream.toml
+-- scenes/                          # Multi-profile compositions
|   +-- movie-night.toml
|   +-- deep-work.toml
+-- rules/                           # Automation rules
|   +-- gaming.toml
|   +-- home-office.toml
+-- schedules/                       # Time-based schedules
|   +-- weekday.toml
|   +-- weekend.toml
+-- templates/                       # User-created profile templates
    +-- my-base.toml

$XDG_DATA_HOME/hypercolor/           # Default: ~/.local/share/hypercolor/
+-- effects/                         # User-installed effects
|   +-- custom/                      # User's own effects
|   +-- community/                   # Downloaded from marketplace
+-- imports/                         # Staging area for imports
+-- backups/                         # Auto-backup before migrations

$XDG_STATE_HOME/hypercolor/          # Default: ~/.local/state/hypercolor/
+-- last-profile.toml                # Active state on last shutdown
+-- device-state.toml                # Last known device connections
+-- profile-revisions.toml           # Optimistic concurrency counters
+-- migration.log                    # Migration audit trail

$XDG_CACHE_HOME/hypercolor/          # Default: ~/.cache/hypercolor/
+-- effect-thumbnails/               # Generated preview images
+-- servo-cache/                     # Servo browser engine cache
+-- discovery-cache.toml             # Cached mDNS/network discovery results

$XDG_RUNTIME_DIR/hypercolor/         # Default: /run/user/$UID/hypercolor/
+-- hypercolor.sock                  # Unix domain socket (IPC)
+-- hypercolor.pid                   # PID file
+-- frame.shm                        # Shared memory for frame data (optional)
```

### 1.2 Windows (AppData)

```
%APPDATA%\hypercolor\                # C:\Users\<user>\AppData\Roaming\hypercolor\
+-- hypercolor.toml
+-- hypercolor.local.toml
+-- devices\
+-- layouts\
+-- profiles\
+-- scenes\
+-- rules\
+-- schedules\
+-- templates\

%LOCALAPPDATA%\hypercolor\           # C:\Users\<user>\AppData\Local\hypercolor\
+-- effects\
+-- imports\
+-- backups\

%LOCALAPPDATA%\hypercolor\state\     # No XDG_STATE_HOME equivalent
+-- last-profile.toml
+-- device-state.toml
+-- profile-revisions.toml
+-- migration.log

%LOCALAPPDATA%\hypercolor\cache\     # No XDG_CACHE_HOME equivalent
+-- effect-thumbnails\
+-- servo-cache\
+-- discovery-cache.toml
```

Windows does not use Unix sockets or PID files. IPC uses a named pipe at `\\.\pipe\hypercolor`.

### 1.3 System-Wide Defaults (Linux Only)

```
/etc/hypercolor/
+-- hypercolor.toml                  # System admin defaults (merged under user config)
+-- devices/                         # Org-wide device presets
```

### 1.4 Resolution Order

Configuration values are resolved in this order. Later sources override earlier ones:

```
/etc/hypercolor/hypercolor.toml          (1) System defaults (Linux)
$CONFIG_DIR/hypercolor.toml              (2) User config
$CONFIG_DIR/hypercolor.local.toml        (3) Machine-local overrides
CLI flags                                (4) --port, --fps, etc.
Environment variables                    (5) HYPERCOLOR_DAEMON__PORT, etc.
```

### 1.5 Path Resolution in Rust

```rust
use std::path::PathBuf;

/// Platform-aware configuration directory resolution.
pub fn config_dir() -> PathBuf {
    #[cfg(target_os = "linux")]
    {
        std::env::var("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                dirs::home_dir()
                    .expect("HOME must be set")
                    .join(".config")
            })
            .join("hypercolor")
    }

    #[cfg(target_os = "windows")]
    {
        dirs::config_dir()
            .expect("APPDATA must be set")
            .join("hypercolor")
    }
}

/// Platform-aware data directory resolution.
pub fn data_dir() -> PathBuf {
    #[cfg(target_os = "linux")]
    {
        std::env::var("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                dirs::home_dir()
                    .expect("HOME must be set")
                    .join(".local/share")
            })
            .join("hypercolor")
    }

    #[cfg(target_os = "windows")]
    {
        dirs::data_local_dir()
            .expect("LOCALAPPDATA must be set")
            .join("hypercolor")
    }
}

/// Platform-aware state directory resolution.
pub fn state_dir() -> PathBuf {
    #[cfg(target_os = "linux")]
    {
        std::env::var("XDG_STATE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                dirs::home_dir()
                    .expect("HOME must be set")
                    .join(".local/state")
            })
            .join("hypercolor")
    }

    #[cfg(target_os = "windows")]
    {
        dirs::data_local_dir()
            .expect("LOCALAPPDATA must be set")
            .join("hypercolor")
            .join("state")
    }
}

/// Platform-aware cache directory resolution.
pub fn cache_dir() -> PathBuf {
    #[cfg(target_os = "linux")]
    {
        std::env::var("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                dirs::home_dir()
                    .expect("HOME must be set")
                    .join(".cache")
            })
            .join("hypercolor")
    }

    #[cfg(target_os = "windows")]
    {
        dirs::cache_dir()
            .expect("LOCALAPPDATA must be set")
            .join("hypercolor")
            .join("cache")
    }
}

/// Platform-aware runtime directory resolution.
///
/// On Linux, uses XDG_RUNTIME_DIR (typically /run/user/$UID).
/// On Windows, returns None -- IPC uses a named pipe instead.
pub fn runtime_dir() -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        std::env::var("XDG_RUNTIME_DIR")
            .map(|d| PathBuf::from(d).join("hypercolor"))
            .ok()
    }

    #[cfg(target_os = "windows")]
    {
        None
    }
}
```

---

## 2. Main Daemon Configuration

**File:** `hypercolor.toml`
**Schema version:** `3` (current)
**Location:** `$CONFIG_DIR/hypercolor/hypercolor.toml`

### 2.1 Complete Annotated Example

```toml
# ~/.config/hypercolor/hypercolor.toml
# Hypercolor daemon configuration
schema_version = 3

# Optional: machine-local overrides loaded after this file.
# Missing include files are silently ignored.
include = ["hypercolor.local.toml"]

# ─── Daemon Core ───────────────────────────────────────────────

[daemon]
# Network binding for the REST API and WebSocket server.
listen_address = "127.0.0.1"        # Default: localhost only
port = 9420                          # Default: 9420
unix_socket = true                   # Enable Unix socket IPC (Linux only)

# Render loop performance.
target_fps = 60                      # Target frames per second
canvas_width = 320                   # Effect canvas width in pixels
canvas_height = 200                  # Effect canvas height in pixels
max_devices = 32                     # Maximum simultaneous device connections

# Logging.
log_level = "info"                   # trace | debug | info | warn | error
log_file = ""                        # Empty = stderr only; path enables file logging

# Startup and shutdown behavior.
start_profile = "last"               # "last" | "default" | <profile name>
shutdown_behavior = "hardware_default"  # "hardware_default" | "off" | "static"
shutdown_color = "#1a1a2e"           # Color when shutdown_behavior = "static"

# ─── Web UI ────────────────────────────────────────────────────

[web]
enabled = true                       # Serve the web UI on the daemon port
open_browser = false                 # Auto-open browser on daemon start
cors_origins = []                    # Additional CORS origins (empty = localhost only)
websocket_fps = 30                   # Preview frame rate for WebSocket clients
auth_enabled = false                 # HTTP basic auth for remote access
# Auth credentials stored in system keyring, NOT here.
# Key: "hypercolor/web-ui/password"

# ─── Effect Engine ─────────────────────────────────────────────

[effect_engine]
preferred_renderer = "auto"          # "auto" | "wgpu" | "servo"
servo_enabled = true                 # Enable Servo path for HTML/Canvas effects
wgpu_backend = "auto"               # "auto" | "vulkan" | "opengl"
extra_effect_dirs = []               # Additional directories to scan for effects
watch_effects = true                 # Hot-reload effects on file change
watch_config = true                  # Hot-reload config on file change

# ─── Audio Input ───────────────────────────────────────────────

[audio]
enabled = true                       # Enable audio capture for reactive effects
device = "default"                   # PulseAudio/PipeWire device name, or "default"
fft_size = 1024                      # FFT window size (256, 512, 1024, 2048, 4096)
smoothing = 0.8                      # FFT smoothing factor (0.0 = raw, 1.0 = frozen)
noise_gate = 0.02                    # Below this level, signal treated as silence
beat_sensitivity = 0.6               # Beat detection threshold (0.0 = never, 1.0 = always)

# ─── Screen Capture ───────────────────────────────────────────

[capture]
enabled = false                      # Enable screen capture for ambient effects
source = "auto"                      # "auto" | "pipewire" | "x11" | "dxgi" (Windows)
capture_fps = 30                     # Capture rate, independent of render FPS
monitor = 0                          # Monitor index (0 = primary)

# ─── Device Discovery ─────────────────────────────────────────

[discovery]
mdns_enabled = true                  # Auto-detect mDNS-advertised devices
scan_interval_secs = 300             # Re-scan interval (seconds)
wled_scan = true                     # Scan for WLED devices
hue_scan = true                      # Scan for Philips Hue bridges

# ─── D-Bus (Linux only) ───────────────────────────────────────

[dbus]
enabled = true                       # Register on the session bus
bus_name = "tech.hyperbliss.hypercolor1"

# ─── TUI ───────────────────────────────────────────────────────

[tui]
theme = "silkcircuit"                # "silkcircuit" | "default" | "minimal"
preview_fps = 15                     # LED preview refresh rate in the TUI
keybindings = "default"              # "default" | "vim" | path to custom keymap

# ─── Feature Flags ─────────────────────────────────────────────

[features]
wasm_plugins = false                 # Experimental: WASM effect plugin system
hue_entertainment = false            # Experimental: Hue Entertainment API streaming
midi_input = false                   # Experimental: MIDI controller input
```

### 2.2 Include Directives

The `include` array specifies additional TOML files to deep-merge on top of the base config. Paths are relative to the directory containing `hypercolor.toml`. Include processing is one level deep -- included files cannot themselves include other files.

```toml
# hypercolor.toml (shared, lives in dotfiles)
schema_version = 3
include = ["hypercolor.local.toml"]

[daemon]
port = 9420
target_fps = 60
```

```toml
# hypercolor.local.toml (machine-specific, NOT in git)
[daemon]
listen_address = "0.0.0.0"          # Open to LAN on this machine

[audio]
device = "alsa_output.usb-Focusrite-monitor"
```

Missing include files are silently ignored. This allows the base config to reference `hypercolor.local.toml` even on machines where that file does not exist.

---

## 3. Profile Format

**File pattern:** `profiles/<name>.toml`
**Schema version:** `2` (current)
**Location:** `$CONFIG_DIR/hypercolor/profiles/`

A profile is a complete, self-contained snapshot of a lighting state. It defines what every zone looks like, with global defaults and per-zone overrides.

### 3.1 Complete Annotated Example

```toml
# ~/.config/hypercolor/profiles/gaming.toml
schema_version = 2

[profile]
id = "gaming"
name = "Gaming Mode"
description = "High-energy reactive lighting for gaming sessions"
author = "Bliss"
created = 2026-03-01T14:30:00Z
modified = 2026-03-15T09:12:00Z
tags = ["gaming", "audio-reactive", "high-energy"]

# Inherit defaults and zones from another profile.
# Empty string or omitted = no inheritance.
base_profile = ""

# ─── Global Defaults ──────────────────────────────────────────
# Applied to all zones unless overridden at the zone level.

[profile.defaults]
brightness = 0.85                    # 0.0 - 1.0, master brightness
saturation = 1.0                     # 0.0 - 1.0, post-process saturation
speed = 1.0                          # Global speed multiplier for all effects
transition_ms = 500                  # Fade time when switching TO this profile

[profile.defaults.audio]
enabled = true                       # Enable audio reactivity for this profile
sensitivity = 0.7                    # 0.0 - 1.0, audio input gain
bass_boost = 1.2                     # Multiplier for bass frequency band
reactive_brightness = true           # Modulate brightness with overall audio level

# ─── Zone Assignments ─────────────────────────────────────────
# Each zone maps a device zone to an effect with parameters.

[[profile.zones]]
zone_id = "hid-16d5-1f01-abc123:channel-0"
effect = "builtin/neon-shift"
layout = "default"

  [profile.zones.params]
  speed = 75
  palette = "Aurora"

  [profile.zones.overrides]
  brightness = 1.0
  saturation = 0.9

[[profile.zones]]
zone_id = "wled-desk-strip:strip"
effect = "community/aurora"
layout = "default"

  [profile.zones.params]
  speed = 40
  color_1 = "#80ffea"
  color_2 = "#e135ff"

  [profile.zones.overrides]
  brightness = 0.6

[[profile.zones]]
zone_id = "razer-huntsman-v2:keyboard"
effect = "native/audio-spectrum"
layout = "default"

  [profile.zones.params]
  color_mode = "gradient"
  gradient_start = "#e135ff"
  gradient_end = "#80ffea"
  smoothing = 0.7

  [profile.zones.overrides]
  brightness = 0.75

  # Zone-specific audio override (replaces profile.defaults.audio for this zone)
  [profile.zones.audio]
  sensitivity = 0.9
  bass_boost = 1.5

# Zones not listed inherit from defaults or remain unchanged.
```

### 3.2 Profile Inheritance

A child profile inherits all defaults and zone assignments from its `base_profile`, then applies its own overrides on top. Zone assignments in the child with the same `zone_id` as the base **fully replace** the base zone (no deep merge of `params` -- that path leads to madness).

```toml
# profiles/gaming-stream.toml
schema_version = 2

[profile]
id = "gaming-stream"
name = "Gaming + Stream"
base_profile = "gaming"              # Inherits from gaming.toml

# Override a global default
[profile.defaults]
brightness = 0.75                    # Dimmer for camera

# Add a zone not in the base profile
[[profile.zones]]
zone_id = "wled-cam-ring:strip"
effect = "builtin/solid-color"

  [profile.zones.params]
  color = "#e135ff"

  [profile.zones.overrides]
  brightness = 0.4                   # Subtle, not distracting on camera
```

**Resolution order:** base defaults < base zones < child defaults < child zones.

### 3.3 Zone ID Format

```
<device-id>:<zone-name>

Device ID construction:
  USB devices:    <backend>-<vid>-<pid>-<serial>       e.g. "hid-16d5-1f01-abc123"
  Network:        <backend>-<hostname-or-ip>            e.g. "wled-desk-strip"
  Hue:            hue-<bridge-id>-<light-id>            e.g. "hue-abc-12"

Zone name:
  The zone's human-readable slug                        e.g. "channel-0", "keyboard", "logo"
```

The daemon silently ignores zone assignments referencing devices that are not connected. This allows a single profile to work across multiple machines with different hardware.

---

## 4. Scene Format

**File pattern:** `scenes/<name>.toml`
**Schema version:** `1` (current)
**Location:** `$CONFIG_DIR/hypercolor/scenes/`

A scene is a composition of profile activations with transition specifications and optional triggers. Where profiles answer "what does each zone look like?", scenes answer "how do we get there, and when?"

### 4.1 Basic Scene

```toml
# ~/.config/hypercolor/scenes/late-night-coding.toml
schema_version = 1

[scene]
id = "01956a3c-7d2e-7f00-a1b2-c3d4e5f6a7b8"
name = "Late Night Coding"
description = "Minimal purple ambience. Easy on the eyes, hard on the bugs."
tags = ["coding", "night", "minimal"]
scope = "pc-only"                    # "full" | "pc-only" | "room-only"
global_brightness = 0.35

[scene.transition]
type = "crossfade"                   # "cut" | "crossfade" | "wipe" | "flash" | "blackout"
duration_ms = 2000
easing = "ease-in-out"               # "linear" | "ease-in" | "ease-out" | "ease-in-out"

# ─── Zone Assignments ─────────────────────────────────────────
# Direct zone assignments (alternative to referencing a profile).

[[assignments]]
zone = "case-fans"
effect = "breathing"
brightness = 0.3
[assignments.parameters]
color = "#e135ff"
speed = 0.4
min_brightness = 0.1

[[assignments]]
zone = "gpu-strimer"
effect = "static"
color = "#80ffea"
brightness = 0.2

[[assignments]]
zone = "ram-sticks"
effect = "static"
color = "#e135ff"
brightness = 0.15

[[assignments]]
zone = "keyboard"
effect = "key-reactive"
[assignments.parameters]
base_color = "#1a1a2e"
press_color = "#e135ff"
decay_ms = 400
```

### 4.2 Scene with Profile Reference

Instead of inline zone assignments, a scene can reference an existing profile and layer transitions on top.

```toml
# ~/.config/hypercolor/scenes/movie-night.toml
schema_version = 1

[scene]
id = "movie-night"
name = "Movie Night"
description = "Dim everything, enable screen ambience on desk strip"
tags = ["media", "ambient", "relaxing"]

[[scene.steps]]
profile = "movie-ambient"
transition = "fade"
transition_ms = 2000
zone_filter = []                     # Empty = all zones in the profile

# Optional: schedule triggers for this scene.
[scene.schedule]
enabled = false
# cron = "0 20 * * FRI"             # Every Friday at 8 PM

# Optional: external triggers for this scene.
[scene.triggers]
dbus_signal = ""                     # e.g. "org.mpris.MediaPlayer2.Playing"
home_assistant_entity = ""           # e.g. "media_player.living_room"
home_assistant_state = ""            # e.g. "playing"
```

### 4.3 Multi-Step Scene (Sequenced Progression)

```toml
# ~/.config/hypercolor/scenes/evening-progression.toml
schema_version = 1

[scene]
id = "evening-progression"
name = "Evening Progression"
description = "Gradual transition from sunset warmth to dim night ambience"
tags = ["evening", "circadian", "progressive"]

[[scene.steps]]
profile = "sunset-warm"
transition = "fade"
transition_ms = 3000
hold_ms = 0                          # 0 = hold until next trigger/delay

[[scene.steps]]
profile = "night-dim"
transition = "fade"
transition_ms = 5000
delay_ms = 1800000                   # 30 minutes after previous step completes
hold_ms = 0

[scene.playback]
mode = "sequential"                  # "sequential" | "manual"
loop = false                         # Restart from step 0 after final step
```

### 4.4 Composed Scene (Layered Overlays)

```toml
# ~/.config/hypercolor/scenes/cozy-music.toml
schema_version = 1

[scene]
id = "cozy-music"
name = "Cozy + Music"
description = "Warm base with audio-reactive WLED overlays"
tags = ["evening", "audio-reactive", "composed"]

[scene.composition]
base = "cozy-evening"

[[scene.composition.overlays]]
scene = "audio-accent"
priority = 5
opacity = 0.7
zones = ["wled-desk-strip:strip", "wled-shelf:strip"]
```

### 4.5 Transition Types

All transition types and their parameters:

| Type | Parameters | Default Duration |
|---|---|---|
| `cut` | (none) | 0ms |
| `crossfade` | `easing` | 1000ms |
| `wipe` | `direction`, `softness`, `easing` | 1500ms |
| `flash` | `flash_color`, `flash_duration_ms`, `easing` | 800ms |
| `blackout` | `hold_ms`, `easing` | 2000ms |

**Wipe directions:** `left`, `right`, `up`, `down`, `radial-in`, `radial-out`, `diagonal` (with optional `angle`).

**Easing functions:** `linear`, `ease-in`, `ease-out`, `ease-in-out`, `cubic-bezier(x1, y1, x2, y2)`, `steps(count, jump)`.

### 4.6 Scope Values

| Scope | Devices Affected |
|---|---|
| `full` | Every device the daemon manages |
| `pc-only` | USB HID and other physically attached devices |
| `room-only` | WLED, Hue -- network/wireless devices |
| `devices` | Explicit device list (array of device IDs) |
| `zones` | Explicit zone list (array of zone IDs) |

Applying a scene with a non-`full` scope leaves unaddressed zones unchanged.

---

## 5. Layout Format

**File pattern:** `layouts/<name>.toml`
**Schema version:** `1` (current)
**Location:** `$CONFIG_DIR/hypercolor/layouts/`

Layouts map device zones to positions on the 320x200 effect canvas. The spatial sampler uses this mapping to convert rendered pixel frames into per-LED color arrays.

### 5.1 Complete Annotated Example

```toml
# ~/.config/hypercolor/layouts/default.toml
schema_version = 1

[layout]
id = "default"
name = "Desktop Setup"
description = "Primary desktop with case, desk strip, and peripherals"
canvas_width = 320
canvas_height = 200
created = 2026-03-01T14:30:00Z
modified = 2026-03-15T09:12:00Z

# Optional background image for the layout editor's visual reference.
background_image = ""

# ─── Zone Placements ───────────────────────────────────────────

# LED strip (case fans, horizontal across middle of canvas)
[[layout.zones]]
device_id = "hid-16d5-1f01-abc123"
zone_name = "channel-0"
x = 0.1                             # Position: normalized 0.0 - 1.0
y = 0.3
width = 0.8
height = 0.05
rotation = 0.0                      # Degrees
topology = "strip"
led_count = 54
direction = "left-to-right"         # "left-to-right" | "right-to-left" |
                                     # "top-to-bottom" | "bottom-to-top"
mirror = false
zigzag = false                       # For matrices: alternate row directions

# WLED desk strip (bottom edge)
[[layout.zones]]
device_id = "wled-desk-strip"
zone_name = "strip"
x = 0.05
y = 0.85
width = 0.9
height = 0.03
rotation = 0.0
topology = "strip"
led_count = 120
direction = "left-to-right"

# Fan ring (center top)
[[layout.zones]]
device_id = "hid-16d5-1f01-abc123"
zone_name = "channel-1"
x = 0.45
y = 0.15
width = 0.1
height = 0.1
rotation = 0.0
topology = "ring"
led_count = 16
direction = "clockwise"              # "clockwise" | "counter-clockwise"

# Keyboard matrix
[[layout.zones]]
device_id = "razer-huntsman-v2"
zone_name = "keyboard"
x = 0.15
y = 0.65
width = 0.7
height = 0.15
rotation = 0.0
topology = "matrix"
led_count = 110
matrix_width = 22
matrix_height = 5
direction = "left-to-right"

# Strimer cable (angled matrix)
[[layout.zones]]
device_id = "hid-16d0-1294-strimer1"
zone_name = "atx-24pin"
x = 0.0
y = 0.0
width = 0.3
height = 0.2
rotation = 15.0                      # Angled to match physical orientation
topology = "matrix"
led_count = 120
matrix_width = 20
matrix_height = 6
direction = "left-to-right"

# Single-LED Hue bulb (custom placement)
[[layout.zones]]
device_id = "hue-bridge1-light12"
zone_name = "bulb"
x = 0.85
y = 0.5
width = 0.05
height = 0.05
rotation = 0.0
topology = "custom"
led_count = 1
led_positions = [[0.5, 0.5]]        # Normalized within zone bounds
```

### 5.2 Topology Types

| Topology | Required Fields | Description |
|---|---|---|
| `strip` | `led_count`, `direction` | Linear LED strip |
| `ring` | `led_count`, `direction` | Circular LED ring (fans, halos) |
| `matrix` | `led_count`, `matrix_width`, `matrix_height`, `direction` | 2D LED grid |
| `single` | `led_count` (always 1) | Single LED point (bulbs) |
| `custom` | `led_count`, `led_positions` | Explicit per-LED coordinates |

### 5.3 Position Coordinates

All positions use normalized coordinates (0.0 to 1.0), relative to the canvas dimensions. This makes layouts resolution-independent. The spatial sampler multiplies by `canvas_width` and `canvas_height` to get pixel coordinates.

---

## 6. Device Configuration

**File pattern:** `devices/<device-id>.toml`
**Schema version:** `1` (current)
**Location:** `$CONFIG_DIR/hypercolor/devices/`

Each discovered device gets its own config file, auto-generated on first detection. Users can refine calibration, rename zones, or disable channels.

### 6.1 USB HID Device

```toml
# ~/.config/hypercolor/devices/hid-16d5-1f01-abc123.toml
schema_version = 1

[device]
id = "hid-16d5-1f01-abc123"
name = "Prism 8 -- Case Fans"       # User-assigned friendly name
backend = "hid"
vendor_id = "16d5"
product_id = "1f01"
serial = "abc123"
enabled = true

# Previous IDs for profile portability when hardware changes.
aliases = []

# ─── Protocol ─────────────────────────────────────────────────

[device.protocol]
color_format = "grb"                 # "rgb" | "grb" | "bgr" | "rgbw" | "grbw"
brightness_multiplier = 0.75         # Hardware-specific brightness cap (0.0 - 1.0)
frame_rate = 60                      # Device-specific FPS target

# ─── Zones ─────────────────────────────────────────────────────

[[device.zones.list]]
name = "channel-0"
led_count = 54                       # Auto-detected or user-set
topology = "strip"
enabled = true

[[device.zones.list]]
name = "channel-1"
led_count = 16
topology = "ring"
enabled = true

[[device.zones.list]]
name = "channel-2"
led_count = 0                        # Empty channel
enabled = false

# ... up to channel-7 for Prism 8

# ─── Calibration ───────────────────────────────────────────────

[device.calibration]
brightness_curve = "gamma"           # "linear" | "gamma" | "cie1931" | "custom"
gamma = 2.2                          # Gamma exponent (when brightness_curve = "gamma")
max_brightness = 0.75                # Hard cap (prevents blinding LEDs)
min_brightness = 0.0                 # Floor (some devices flicker below a threshold)
white_point = [255, 240, 220]        # Per-channel RGB max values (corrects tint)
color_temp_k = 6500                  # Target color temperature for whites
gamma_rgb = [2.2, 2.2, 2.2]         # Per-channel gamma fine-tuning
led_density = 60                     # LEDs per meter (for spatial calculations)
power_limit_watts = 0.0              # 0 = unlimited; non-zero enables power estimation
power_per_led_mw = 60                # Milliwatts per LED at full white
voltage = 5.0                        # Supply voltage

# Custom brightness curve LUT (when brightness_curve = "custom").
# Key points are linearly interpolated. Format: [[input, output], ...]
[device.calibration.custom_curve]
points = [[0, 0], [64, 10], [128, 50], [192, 130], [255, 255]]

# ─── Shutdown ──────────────────────────────────────────────────

[device.shutdown]
behavior = "static"                  # "off" | "static" | "hardware_default"
color = "#1a1a2e"

# ─── Attachment Slots & Bindings ──────────────────────────────

# Slots are controller-facing attachment points. They are typically auto-derived
# from discovered zones, but users can refine names, categories, and allowed
# templates.
[[device.attachments.slots]]
id = "atx-strimer"
name = "ATX Port"
led_start = 0
led_count = 120
suggested_categories = ["strimer", "matrix"]
allow_custom = true

[[device.attachments.slots]]
id = "gpu-strimer"
name = "GPU Port"
led_start = 120
led_count = 162
suggested_categories = ["strimer", "matrix"]
allowed_templates = ["strimer-gpu-dual-8", "strimer-gpu-triple-8"]
allow_custom = true

# Bindings associate a slot with either a built-in preset or a user-authored
# custom attachment template.
[[device.attachments.bindings]]
slot_id = "atx-strimer"
template_id = "strimer-atx-24pin"
name = "24-pin ATX Cable"
enabled = true

[[device.attachments.bindings]]
slot_id = "gpu-strimer"
template_id = "my-custom-gpu-sleeve"
name = "4090 GPU Sleeve"
enabled = true
```

### 6.2 WLED Network Device

```toml
# ~/.config/hypercolor/devices/wled-desk-strip.toml
schema_version = 1

[device]
id = "wled-desk-strip"
name = "Desk Underglow"
backend = "wled"
hostname = "wled-desk.local"         # mDNS name
ip = "192.168.1.42"                  # Resolved / static IP fallback
enabled = true
aliases = ["wled-192.168.1.42"]      # Old ID before mDNS rename

[device.protocol]
transport = "ddp"                    # "ddp" | "e131"
color_format = "rgb"
brightness_multiplier = 1.0

# WLED-specific protocol settings.
[device.protocol.wled]
segment = 0                          # WLED segment to control (0 = all)

[[device.zones.list]]
name = "strip"
led_count = 120
topology = "strip"
enabled = true

[device.calibration]
brightness_curve = "gamma"
gamma = 2.8                          # LED strips typically need higher gamma
max_brightness = 1.0
min_brightness = 0.0
white_point = [255, 255, 255]
color_temp_k = 6500
gamma_rgb = [2.8, 2.8, 2.8]
led_density = 60
power_limit_watts = 0.0
power_per_led_mw = 60
voltage = 5.0

[device.shutdown]
behavior = "off"
```

### 6.3 Philips Hue Device

```toml
# ~/.config/hypercolor/devices/hue-bridge1-light12.toml
schema_version = 1

[device]
id = "hue-bridge1-light12"
name = "Desk Lamp (Hue)"
backend = "hue"
bridge_id = "bridge1"
light_id = "12"
enabled = true

[device.protocol]
transport = "entertainment"          # "entertainment" | "rest"
color_format = "rgb"
# API token stored in system keyring at "hypercolor/hue/bridge1/token"

[[device.zones.list]]
name = "bulb"
led_count = 1
topology = "single"
enabled = true

[device.calibration]
gamut = "C"                          # Hue color gamut: "A" | "B" | "C"
brightness_curve = "gamma"
gamma = 2.2
max_brightness = 1.0
min_brightness = 0.01                # Hue bulbs flicker below certain levels
white_point = [255, 255, 255]
color_temp_k = 6500
transition_time_ms = 100             # Hue-specific transition smoothing
gamma_rgb = [2.2, 2.2, 2.2]
power_limit_watts = 0.0
power_per_led_mw = 0
voltage = 0.0

[device.shutdown]
behavior = "off"
```

---

## 7. Automation Rules

**File pattern:** `rules/<name>.toml`
**Schema version:** `1` (current)
**Location:** `$CONFIG_DIR/hypercolor/rules/`

Rules connect triggers (events) to actions (scene changes, brightness adjustments) with optional conditions and constraints.

### 7.1 Complete Gaming Rules Example

```toml
# ~/.config/hypercolor/rules/gaming.toml
schema_version = 1

# ─── Rule: Game Launch ────────────────────────────────────────

[[rules]]
name = "Game Launch -> Gaming Mode"
description = "Switch to reactive gaming scene when a game starts"
enabled = true
priority = 50                        # Active tier (50-69)
cooldown = "30s"                     # Don't re-trigger within this window
tags = ["gaming"]

[rules.trigger]
type = "event"
source = "app"
event_type = "launched"
filter = { category = "game" }

# Only fire during evening/night hours.
[rules.conditions]
time_range = { start = "17:00", end = "03:00" }

[rules.action]
type = "activate_scene"
scene = "gaming-reactive"
transition = { type = "flash", flash_color = "#e135ff", flash_duration = "200ms", duration = "800ms" }

# ─── Rule: Game Exit ──────────────────────────────────────────

[[rules]]
name = "Game Exit -> Restore"
enabled = true
priority = 50
tags = ["gaming"]

[rules.trigger]
type = "event"
source = "app"
event_type = "exited"
filter = { category = "game" }

[rules.action]
type = "restore_previous_scene"
transition = { type = "crossfade", duration = "2s" }

# ─── Rule: Screen Lock ────────────────────────────────────────

[[rules]]
name = "Screen Lock -> Dim"
enabled = true
priority = 30                        # Normal tier
tags = ["system"]

[rules.trigger]
type = "event"
source = "desktop"
event_type = "screen_locked"

[rules.action]
type = "set_brightness"
brightness = 0.05
transition_ms = 3000

# ─── Rule: Screen Unlock ──────────────────────────────────────

[[rules]]
name = "Screen Unlock -> Restore"
enabled = true
priority = 30
tags = ["system"]

[rules.trigger]
type = "event"
source = "desktop"
event_type = "screen_unlocked"

[rules.action]
type = "set_brightness"
brightness = 1.0
transition_ms = 1000

# ─── Rule: Video Call ──────────────────────────────────────────

[[rules]]
name = "Video Call -> Calm"
description = "Reduce distracting lighting during video calls"
enabled = true
priority = 60                        # Important tier
tags = ["productivity"]

[rules.trigger]
type = "any"
events = [
    { source = "app", event_type = "launched", filter = { name = "zoom" } },
    { source = "app", event_type = "launched", filter = { name = "teams" } },
]

[rules.action]
type = "activate_scene"
scene = "video-call-calm"
transition = { type = "crossfade", duration = "1s" }
```

### 7.2 Home Automation Rules Example

```toml
# ~/.config/hypercolor/rules/home-office.toml
schema_version = 1

[[rules]]
name = "Motion Detected -> Lights On"
enabled = true
priority = 40
cooldown = "5m"
tags = ["smart-home"]

[rules.trigger]
type = "event"
source = "ha"
event_type = "entity_changed"
filter = { entity_id = "binary_sensor.office_motion", state = "on" }

[rules.conditions]
time_range = { start = "18:00", end = "23:59" }
current_scene = { not = "gaming-reactive" }

[rules.action]
type = "activate_scene"
scene = "evening-warm"
transition = { type = "crossfade", duration = "2s" }

# ─────────────────────────────────────────────────

[[rules]]
name = "Doorbell Ring -> Flash Alert"
enabled = true
priority = 80                        # Important: visible even during gaming
cooldown = "10s"
tags = ["smart-home", "alerts"]

[rules.trigger]
type = "event"
source = "external"
event_type = "doorbell.ring"

[rules.action]
type = "temporary_overlay"
scene = "alert-flash-white"
duration = "5s"
transition = { type = "flash", flash_color = "#ffffff", flash_duration = "300ms", duration = "500ms" }
```

### 7.3 Priority Tiers

```
  0-29    Background      Circadian rhythm, ambient adjustments
 30-49    Normal          Screen lock, idle timeouts, time-based
 50-69    Active          App launches, games, media playback
 70-89    Important       Video calls, stream events, alerts
 90-99    Critical        Manual override, safety notifications
100+      System          Fail-safe, seizure protection
```

Resolution: highest priority wins. Ties broken by most-recently-triggered. Active rule IDs are tracked in a priority stack for `restore_previous_scene` operations.

### 7.4 Trigger Types

| Type | Description | Example |
|---|---|---|
| `event` | Single event match from a source | App launched, screen locked |
| `any` | Any of a list of events fires | Zoom OR Teams launched |
| `all` | All events must fire (unordered) | Game + headphones connected |
| `sequence` | Ordered events within a time window | USB connect then app launch within 10s |

### 7.5 Action Types

| Type | Description |
|---|---|
| `activate_scene` | Switch to a named scene |
| `restore_previous_scene` | Pop the priority stack |
| `set_brightness` | Adjust global brightness without changing scene |
| `set_zone_effect` | Change a single zone's effect |
| `temporary_overlay` | Apply a scene overlay that auto-removes after duration |
| `sequence` | Execute multiple actions in order |
| `parallel` | Execute multiple actions simultaneously |
| `delay` | Wait before next action (within a sequence) |
| `webhook` | POST to an external URL |
| `mqtt_publish` | Publish a message to an MQTT topic |
| `noop` | No-op (useful for testing rules) |

---

## 8. Rust Types

All config structs use `serde::Serialize` and `serde::Deserialize` with `#[serde(default)]` for forward/backward compatibility.

### 8.1 Top-Level Config

```rust
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Root configuration loaded from `hypercolor.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HypercolorConfig {
    /// Schema version for migration tracking.
    pub schema_version: u32,

    /// Additional TOML files to merge (relative paths).
    #[serde(default)]
    pub include: Vec<String>,

    #[serde(default)]
    pub daemon: DaemonConfig,

    #[serde(default)]
    pub web: WebConfig,

    #[serde(default)]
    pub effect_engine: EffectEngineConfig,

    #[serde(default)]
    pub audio: AudioConfig,

    #[serde(default)]
    pub capture: CaptureConfig,

    #[serde(default)]
    pub discovery: DiscoveryConfig,

    #[serde(default)]
    pub dbus: DbusConfig,

    #[serde(default)]
    pub tui: TuiConfig,

    #[serde(default)]
    pub features: FeatureFlags,
}
```

### 8.2 Daemon Config

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonConfig {
    #[serde(default = "defaults::listen_address")]
    pub listen_address: String,

    #[serde(default = "defaults::port")]
    pub port: u16,

    #[serde(default = "defaults::bool_true")]
    pub unix_socket: bool,

    #[serde(default = "defaults::target_fps")]
    pub target_fps: u32,

    #[serde(default = "defaults::canvas_width")]
    pub canvas_width: u32,

    #[serde(default = "defaults::canvas_height")]
    pub canvas_height: u32,

    #[serde(default = "defaults::max_devices")]
    pub max_devices: u32,

    #[serde(default = "defaults::log_level")]
    pub log_level: LogLevel,

    #[serde(default)]
    pub log_file: String,

    #[serde(default = "defaults::start_profile")]
    pub start_profile: String,

    #[serde(default = "defaults::shutdown_behavior")]
    pub shutdown_behavior: ShutdownBehavior,

    #[serde(default = "defaults::shutdown_color")]
    pub shutdown_color: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShutdownBehavior {
    HardwareDefault,
    Off,
    Static,
}
```

### 8.3 Web Config

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebConfig {
    #[serde(default = "defaults::bool_true")]
    pub enabled: bool,

    #[serde(default)]
    pub open_browser: bool,

    #[serde(default)]
    pub cors_origins: Vec<String>,

    #[serde(default = "defaults::websocket_fps")]
    pub websocket_fps: u32,

    #[serde(default)]
    pub auth_enabled: bool,
}
```

### 8.4 Audio Config

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioConfig {
    #[serde(default = "defaults::bool_true")]
    pub enabled: bool,

    #[serde(default = "defaults::audio_device")]
    pub device: String,

    #[serde(default = "defaults::fft_size")]
    pub fft_size: u32,

    #[serde(default = "defaults::smoothing")]
    pub smoothing: f32,

    #[serde(default = "defaults::noise_gate")]
    pub noise_gate: f32,

    #[serde(default = "defaults::beat_sensitivity")]
    pub beat_sensitivity: f32,
}
```

### 8.5 Capture Config

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureConfig {
    #[serde(default)]
    pub enabled: bool,

    #[serde(default = "defaults::capture_source")]
    pub source: String,

    #[serde(default = "defaults::capture_fps")]
    pub capture_fps: u32,

    #[serde(default)]
    pub monitor: u32,
}
```

### 8.6 TUI Config

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TuiConfig {
    #[serde(default = "defaults::tui_theme")]
    pub theme: String,

    #[serde(default = "defaults::preview_fps")]
    pub preview_fps: u32,

    #[serde(default = "defaults::keybindings")]
    pub keybindings: String,
}
```

### 8.7 Discovery Config

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryConfig {
    #[serde(default = "defaults::bool_true")]
    pub mdns_enabled: bool,

    #[serde(default = "defaults::scan_interval")]
    pub scan_interval_secs: u64,

    #[serde(default = "defaults::bool_true")]
    pub wled_scan: bool,

    #[serde(default = "defaults::bool_true")]
    pub hue_scan: bool,
}
```

### 8.8 Effect Engine Config

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectEngineConfig {
    #[serde(default = "defaults::auto_string")]
    pub preferred_renderer: String,

    #[serde(default = "defaults::bool_true")]
    pub servo_enabled: bool,

    #[serde(default = "defaults::auto_string")]
    pub wgpu_backend: String,

    #[serde(default)]
    pub extra_effect_dirs: Vec<PathBuf>,

    #[serde(default = "defaults::bool_true")]
    pub watch_effects: bool,

    #[serde(default = "defaults::bool_true")]
    pub watch_config: bool,
}
```

### 8.9 D-Bus Config

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbusConfig {
    #[serde(default = "defaults::bool_true")]
    pub enabled: bool,

    #[serde(default = "defaults::bus_name")]
    pub bus_name: String,
}
```

### 8.10 Feature Flags

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FeatureFlags {
    #[serde(default)]
    pub wasm_plugins: bool,

    #[serde(default)]
    pub hue_entertainment: bool,

    #[serde(default)]
    pub midi_input: bool,
}
```

### 8.11 Profile Types

```rust
/// Root of a profile TOML file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileFile {
    pub schema_version: u32,
    pub profile: Profile,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub id: String,
    pub name: String,

    #[serde(default)]
    pub description: String,

    #[serde(default)]
    pub author: String,

    #[serde(default)]
    pub created: Option<DateTime<Utc>>,

    #[serde(default)]
    pub modified: Option<DateTime<Utc>>,

    #[serde(default)]
    pub tags: Vec<String>,

    #[serde(default)]
    pub base_profile: String,

    #[serde(default)]
    pub defaults: ProfileDefaults,

    #[serde(default)]
    pub zones: Vec<ProfileZone>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileDefaults {
    #[serde(default = "defaults::brightness")]
    pub brightness: f32,

    #[serde(default = "defaults::saturation")]
    pub saturation: f32,

    #[serde(default = "defaults::speed_one")]
    pub speed: f32,

    #[serde(default = "defaults::transition_ms")]
    pub transition_ms: u64,

    #[serde(default)]
    pub audio: ProfileAudioDefaults,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileAudioDefaults {
    #[serde(default)]
    pub enabled: bool,

    #[serde(default = "defaults::audio_sensitivity")]
    pub sensitivity: f32,

    #[serde(default = "defaults::bass_boost")]
    pub bass_boost: f32,

    #[serde(default)]
    pub reactive_brightness: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileZone {
    pub zone_id: String,
    pub effect: String,

    #[serde(default = "defaults::default_layout")]
    pub layout: String,

    #[serde(default)]
    pub params: toml::Table,

    #[serde(default)]
    pub overrides: ZoneOverrides,

    #[serde(default)]
    pub audio: Option<ProfileAudioDefaults>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ZoneOverrides {
    pub brightness: Option<f32>,
    pub saturation: Option<f32>,
    pub speed: Option<f32>,
}
```

### 8.12 Scene Types

```rust
/// Root of a scene TOML file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneFile {
    pub schema_version: u32,
    pub scene: SceneDefinition,

    #[serde(default)]
    pub assignments: Vec<SceneAssignment>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneDefinition {
    pub id: String,
    pub name: String,

    #[serde(default)]
    pub description: String,

    #[serde(default)]
    pub tags: Vec<String>,

    #[serde(default = "defaults::scope_full")]
    pub scope: String,

    #[serde(default = "defaults::brightness")]
    pub global_brightness: f32,

    #[serde(default)]
    pub transition: TransitionSpec,

    #[serde(default)]
    pub steps: Vec<SceneStep>,

    #[serde(default)]
    pub composition: Option<SceneComposition>,

    #[serde(default)]
    pub schedule: SceneSchedule,

    #[serde(default)]
    pub triggers: SceneTriggers,

    #[serde(default)]
    pub playback: PlaybackConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransitionSpec {
    #[serde(default = "defaults::transition_type")]
    #[serde(rename = "type")]
    pub transition_type: String,

    #[serde(default = "defaults::transition_duration")]
    pub duration_ms: u64,

    #[serde(default = "defaults::easing")]
    pub easing: String,

    // Wipe-specific
    #[serde(default)]
    pub direction: Option<String>,

    #[serde(default)]
    pub softness: Option<f32>,

    // Flash-specific
    #[serde(default)]
    pub flash_color: Option<String>,

    #[serde(default)]
    pub flash_duration_ms: Option<u64>,

    // Blackout-specific
    #[serde(default)]
    pub hold_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneStep {
    pub profile: String,

    #[serde(default = "defaults::transition_type")]
    pub transition: String,

    #[serde(default = "defaults::transition_duration")]
    pub transition_ms: u64,

    #[serde(default)]
    pub zone_filter: Vec<String>,

    #[serde(default)]
    pub hold_ms: u64,

    #[serde(default)]
    pub delay_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneAssignment {
    pub zone: String,
    pub effect: String,

    #[serde(default)]
    pub brightness: Option<f32>,

    #[serde(default)]
    pub color: Option<String>,

    #[serde(default)]
    pub parameters: toml::Table,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneComposition {
    pub base: String,

    #[serde(default)]
    pub overlays: Vec<SceneOverlay>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneOverlay {
    pub scene: String,

    #[serde(default = "defaults::priority_five")]
    pub priority: u8,

    #[serde(default = "defaults::opacity_one")]
    pub opacity: f32,

    #[serde(default)]
    pub zones: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SceneSchedule {
    #[serde(default)]
    pub enabled: bool,

    #[serde(default)]
    pub cron: Option<String>,

    #[serde(default)]
    pub condition: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SceneTriggers {
    #[serde(default)]
    pub dbus_signal: String,

    #[serde(default)]
    pub home_assistant_entity: String,

    #[serde(default)]
    pub home_assistant_state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaybackConfig {
    #[serde(default = "defaults::playback_sequential")]
    pub mode: String,

    #[serde(default)]
    pub r#loop: bool,
}
```

### 8.13 Device Types

```rust
/// Root of a device config TOML file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceFile {
    pub schema_version: u32,
    pub device: DeviceConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceConfig {
    pub id: String,
    pub name: String,
    pub backend: String,

    #[serde(default)]
    pub vendor_id: Option<String>,

    #[serde(default)]
    pub product_id: Option<String>,

    #[serde(default)]
    pub serial: Option<String>,

    #[serde(default)]
    pub hostname: Option<String>,

    #[serde(default)]
    pub ip: Option<String>,

    #[serde(default)]
    pub bridge_id: Option<String>,

    #[serde(default)]
    pub light_id: Option<String>,

    #[serde(default = "defaults::bool_true")]
    pub enabled: bool,

    #[serde(default)]
    pub aliases: Vec<String>,

    #[serde(default)]
    pub protocol: DeviceProtocol,

    #[serde(default)]
    pub zones: DeviceZones,

    #[serde(default)]
    pub calibration: DeviceCalibration,

    #[serde(default)]
    pub shutdown: DeviceShutdown,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeviceProtocol {
    #[serde(default = "defaults::color_format_rgb")]
    pub color_format: String,

    #[serde(default = "defaults::brightness_multiplier")]
    pub brightness_multiplier: f32,

    #[serde(default = "defaults::target_fps")]
    pub frame_rate: u32,

    #[serde(default)]
    pub transport: Option<String>,

    #[serde(default)]
    pub wled: Option<WledProtocol>,

    #[serde(default)]
    pub hue: Option<HueProtocol>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WledProtocol {
    #[serde(default)]
    pub segment: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HueProtocol {
    #[serde(default)]
    pub bridge_id: String,
    // Token stored in system keyring, NOT in config.
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeviceZones {
    #[serde(default)]
    pub list: Vec<DeviceZoneEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceZoneEntry {
    pub name: String,

    #[serde(default)]
    pub led_count: u32,

    #[serde(default = "defaults::topology_strip")]
    pub topology: String,

    #[serde(default = "defaults::bool_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceCalibration {
    #[serde(default = "defaults::brightness_curve")]
    pub brightness_curve: String,

    #[serde(default = "defaults::gamma")]
    pub gamma: f32,

    #[serde(default = "defaults::max_brightness")]
    pub max_brightness: f32,

    #[serde(default)]
    pub min_brightness: f32,

    #[serde(default = "defaults::white_point")]
    pub white_point: [u8; 3],

    #[serde(default = "defaults::color_temp")]
    pub color_temp_k: u32,

    #[serde(default = "defaults::gamma_rgb")]
    pub gamma_rgb: [f32; 3],

    #[serde(default = "defaults::led_density")]
    pub led_density: u32,

    #[serde(default)]
    pub power_limit_watts: f32,

    #[serde(default = "defaults::power_per_led")]
    pub power_per_led_mw: u32,

    #[serde(default = "defaults::voltage")]
    pub voltage: f32,

    #[serde(default)]
    pub gamut: Option<String>,

    #[serde(default)]
    pub transition_time_ms: Option<u64>,

    #[serde(default)]
    pub custom_curve: Option<CustomCurve>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CustomCurve {
    pub points: Vec<[u8; 2]>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceShutdown {
    #[serde(default = "defaults::shutdown_hardware")]
    pub behavior: String,

    #[serde(default)]
    pub color: Option<String>,
}
```

### 8.14 Automation Rule Types

```rust
/// Root of a rules TOML file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RulesFile {
    #[serde(default)]
    pub schema_version: u32,

    pub rules: Vec<AutomationRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutomationRule {
    pub name: String,

    #[serde(default)]
    pub description: Option<String>,

    #[serde(default = "defaults::bool_true")]
    pub enabled: bool,

    #[serde(default = "defaults::priority_default")]
    pub priority: u8,

    #[serde(default)]
    pub cooldown: Option<String>,

    #[serde(default)]
    pub tags: Vec<String>,

    pub trigger: TriggerExpr,

    #[serde(default)]
    pub conditions: RuleConditions,

    pub action: ActionExpr,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TriggerExpr {
    Event {
        source: String,
        event_type: String,
        #[serde(default)]
        filter: Option<toml::Table>,
    },
    Any {
        events: Vec<TriggerExpr>,
    },
    All {
        events: Vec<TriggerExpr>,
    },
    Sequence {
        events: Vec<TriggerExpr>,
        within: String,
    },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RuleConditions {
    #[serde(default)]
    pub time_range: Option<TimeRange>,

    #[serde(default)]
    pub days: Option<String>,

    #[serde(default)]
    pub current_scene: Option<toml::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeRange {
    pub start: String,
    pub end: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ActionExpr {
    ActivateScene {
        scene: String,
        #[serde(default)]
        transition: Option<toml::Table>,
    },
    RestorePreviousScene {
        #[serde(default)]
        transition: Option<toml::Table>,
    },
    SetBrightness {
        brightness: f32,
        #[serde(default)]
        transition_ms: u64,
    },
    SetZoneEffect {
        zone: String,
        effect: String,
        #[serde(default)]
        parameters: toml::Table,
    },
    TemporaryOverlay {
        scene: String,
        duration: String,
        #[serde(default)]
        transition: Option<toml::Table>,
    },
    Sequence {
        actions: Vec<ActionExpr>,
    },
    Parallel {
        actions: Vec<ActionExpr>,
    },
    Delay {
        duration: String,
    },
    Webhook {
        url: String,
        #[serde(default = "defaults::http_post")]
        method: String,
        #[serde(default)]
        body: Option<toml::Table>,
    },
    MqttPublish {
        topic: String,
        payload: String,
    },
    Noop,
}
```

### 8.15 Config Manager

```rust
use arc_swap::ArcSwap;
use std::sync::atomic::AtomicU64;
use tokio::sync::broadcast;

/// Central configuration authority. All mutations flow through this struct.
pub struct ConfigManager {
    /// The live config -- atomically swappable, lock-free reads.
    config: ArcSwap<HypercolorConfig>,

    /// Monotonic revision counter for optimistic concurrency.
    revision: AtomicU64,

    /// Filesystem watcher for external edits.
    watcher: notify::RecommendedWatcher,

    /// Event bus for broadcasting changes.
    bus: broadcast::Sender<ConfigEvent>,
}

pub enum ConfigEvent {
    ConfigChanged {
        section: ConfigSection,
        revision: u64,
        source: ChangeSource,
    },
    ProfileChanged {
        id: String,
        action: ChangeAction,
        revision: u64,
    },
    DeviceConfigChanged {
        device_id: String,
        revision: u64,
    },
    LayoutChanged {
        layout_id: String,
        revision: u64,
    },
    ParseError {
        path: std::path::PathBuf,
        error: String,
    },
}

pub enum ConfigSection {
    Daemon,
    Web,
    Audio,
    Capture,
    Discovery,
    EffectEngine,
    Dbus,
    Tui,
    Features,
}

pub enum ChangeSource {
    WebUI,
    TUI,
    CLI,
    DBus,
    FileSystem,
    Migration,
    API,
}

pub enum ChangeAction {
    Created,
    Updated,
    Deleted,
}
```

---

## 9. Default Values

Every field has a compile-time default. A fresh install with zero config files starts the daemon with these values.

### 9.1 Default Value Table

| Section | Field | Default | Notes |
|---|---|---|---|
| **daemon** | `listen_address` | `"127.0.0.1"` | Localhost only |
| | `port` | `9420` | |
| | `unix_socket` | `true` | Linux only |
| | `target_fps` | `60` | |
| | `canvas_width` | `320` | LightScript standard |
| | `canvas_height` | `200` | LightScript standard |
| | `max_devices` | `32` | |
| | `log_level` | `"info"` | |
| | `log_file` | `""` | stderr only |
| | `start_profile` | `"last"` | Resume last session |
| | `shutdown_behavior` | `"hardware_default"` | |
| | `shutdown_color` | `"#1a1a2e"` | |
| **web** | `enabled` | `true` | |
| | `open_browser` | `false` | |
| | `cors_origins` | `[]` | localhost only |
| | `websocket_fps` | `30` | |
| | `auth_enabled` | `false` | |
| **effect_engine** | `preferred_renderer` | `"auto"` | |
| | `servo_enabled` | `true` | |
| | `wgpu_backend` | `"auto"` | |
| | `extra_effect_dirs` | `[]` | |
| | `watch_effects` | `true` | |
| | `watch_config` | `true` | |
| **audio** | `enabled` | `true` | |
| | `device` | `"default"` | System default |
| | `fft_size` | `1024` | |
| | `smoothing` | `0.8` | |
| | `noise_gate` | `0.02` | |
| | `beat_sensitivity` | `0.6` | |
| **capture** | `enabled` | `false` | Off by default |
| | `source` | `"auto"` | |
| | `capture_fps` | `30` | |
| | `monitor` | `0` | Primary |
| **discovery** | `mdns_enabled` | `true` | |
| | `scan_interval_secs` | `300` | 5 minutes |
| | `wled_scan` | `true` | |
| | `hue_scan` | `true` | |
| **dbus** | `enabled` | `true` | Linux only |
| | `bus_name` | `"tech.hyperbliss.hypercolor1"` | |
| **tui** | `theme` | `"silkcircuit"` | |
| | `preview_fps` | `15` | |
| | `keybindings` | `"default"` | |
| **features** | `wasm_plugins` | `false` | |
| | `hue_entertainment` | `false` | |
| | `midi_input` | `false` | |

### 9.2 Profile Defaults

| Field | Default | Notes |
|---|---|---|
| `brightness` | `0.85` | |
| `saturation` | `1.0` | Full saturation |
| `speed` | `1.0` | Normal speed |
| `transition_ms` | `500` | |
| `audio.enabled` | `false` | |
| `audio.sensitivity` | `0.7` | |
| `audio.bass_boost` | `1.0` | No boost |
| `audio.reactive_brightness` | `false` | |

### 9.3 Device Calibration Defaults

| Field | Default | Notes |
|---|---|---|
| `brightness_curve` | `"gamma"` | |
| `gamma` | `2.2` | sRGB standard |
| `max_brightness` | `1.0` | No cap |
| `min_brightness` | `0.0` | |
| `white_point` | `[255, 255, 255]` | Pure white |
| `color_temp_k` | `6500` | Daylight |
| `gamma_rgb` | `[2.2, 2.2, 2.2]` | Uniform |
| `led_density` | `60` | LEDs/meter |
| `power_limit_watts` | `0.0` | Unlimited |
| `power_per_led_mw` | `60` | WS2812B typical |
| `voltage` | `5.0` | |

### 9.4 Vendor-Specific Device Defaults

When a device is first detected, vendor-specific defaults override the generic defaults:

| Device | `color_format` | `brightness_multiplier` | `gamma` | `frame_rate` |
|---|---|---|---|---|
| PrismRGB Prism 8 (16D5:1F01) | `grb` | `0.75` | `2.2` | `60` |
| PrismRGB Prism S (16D0:1294) | `rgb` | `0.50` | `2.2` | `33` |
| WLED (any) | `rgb` | `1.0` | `2.8` | `60` |
| Hue (any) | `rgb` | `1.0` | `2.2` | `25` |
| Generic/Unknown | `rgb` | `1.0` | `2.2` | `60` |

### 9.5 Default Value Functions

```rust
/// All default value functions referenced by `#[serde(default = "...")]`.
mod defaults {
    pub fn listen_address() -> String { "127.0.0.1".into() }
    pub fn port() -> u16 { 9420 }
    pub fn target_fps() -> u32 { 60 }
    pub fn canvas_width() -> u32 { 320 }
    pub fn canvas_height() -> u32 { 200 }
    pub fn max_devices() -> u32 { 32 }
    pub fn log_level() -> super::LogLevel { super::LogLevel::Info }
    pub fn start_profile() -> String { "last".into() }
    pub fn shutdown_behavior() -> super::ShutdownBehavior {
        super::ShutdownBehavior::HardwareDefault
    }
    pub fn shutdown_color() -> String { "#1a1a2e".into() }
    pub fn websocket_fps() -> u32 { 30 }
    pub fn audio_device() -> String { "default".into() }
    pub fn fft_size() -> u32 { 1024 }
    pub fn smoothing() -> f32 { 0.8 }
    pub fn noise_gate() -> f32 { 0.02 }
    pub fn beat_sensitivity() -> f32 { 0.6 }
    pub fn capture_source() -> String { "auto".into() }
    pub fn capture_fps() -> u32 { 30 }
    pub fn scan_interval() -> u64 { 300 }
    pub fn bus_name() -> String { "tech.hyperbliss.hypercolor1".into() }
    pub fn tui_theme() -> String { "silkcircuit".into() }
    pub fn preview_fps() -> u32 { 15 }
    pub fn keybindings() -> String { "default".into() }
    pub fn auto_string() -> String { "auto".into() }
    pub fn bool_true() -> bool { true }

    // Profile defaults
    pub fn brightness() -> f32 { 0.85 }
    pub fn saturation() -> f32 { 1.0 }
    pub fn speed_one() -> f32 { 1.0 }
    pub fn transition_ms() -> u64 { 500 }
    pub fn audio_sensitivity() -> f32 { 0.7 }
    pub fn bass_boost() -> f32 { 1.0 }
    pub fn default_layout() -> String { "default".into() }

    // Scene defaults
    pub fn scope_full() -> String { "full".into() }
    pub fn transition_type() -> String { "crossfade".into() }
    pub fn transition_duration() -> u64 { 1000 }
    pub fn easing() -> String { "ease-in-out".into() }
    pub fn playback_sequential() -> String { "sequential".into() }
    pub fn priority_five() -> u8 { 5 }
    pub fn opacity_one() -> f32 { 1.0 }

    // Device defaults
    pub fn color_format_rgb() -> String { "rgb".into() }
    pub fn brightness_multiplier() -> f32 { 1.0 }
    pub fn brightness_curve() -> String { "gamma".into() }
    pub fn gamma() -> f32 { 2.2 }
    pub fn max_brightness() -> f32 { 1.0 }
    pub fn white_point() -> [u8; 3] { [255, 255, 255] }
    pub fn color_temp() -> u32 { 6500 }
    pub fn gamma_rgb() -> [f32; 3] { [2.2, 2.2, 2.2] }
    pub fn led_density() -> u32 { 60 }
    pub fn power_per_led() -> u32 { 60 }
    pub fn voltage() -> f32 { 5.0 }
    pub fn topology_strip() -> String { "strip".into() }
    pub fn shutdown_hardware() -> String { "hardware_default".into() }

    // Rule defaults
    pub fn priority_default() -> u8 { 50 }
    pub fn http_post() -> String { "POST".into() }
}
```

---

## 10. Schema Versioning & Migration

### 10.1 Schema Version Tracking

Every config file carries a `schema_version` integer at the top level. The daemon validates this on load and auto-migrates when necessary.

```toml
schema_version = 3                   # Must be present in every config file
```

Current schema versions by config kind:

| Config Kind | Current Version | Notes |
|---|---|---|
| Main (`hypercolor.toml`) | `3` | v3 added `[features]` section |
| Profile | `2` | v2 added per-zone `[audio]` overrides |
| Scene | `1` | Initial schema |
| Layout | `1` | Initial schema |
| Device | `1` | Initial schema |
| Rules | `1` | Initial schema |

### 10.2 Migration Engine

```rust
/// Manages schema migrations for all config file types.
pub struct MigrationEngine {
    migrations: BTreeMap<(ConfigKind, u32, u32), Box<dyn Migration>>,
}

pub trait Migration: Send + Sync {
    fn from_version(&self) -> u32;
    fn to_version(&self) -> u32;
    fn config_kind(&self) -> ConfigKind;

    /// Transform the TOML document in place using `toml_edit` for
    /// format-preserving modifications.
    fn migrate(&self, doc: &mut toml_edit::DocumentMut) -> Result<()>;

    fn description(&self) -> &str;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ConfigKind {
    Main,
    Device,
    Layout,
    Profile,
    Scene,
    Rules,
}
```

Migrations form a linear chain per config kind. The engine walks from the file's declared version to the latest:

```
Profile v1 --migrate_v1_to_v2--> Profile v2 (current)
Main v1 --migrate_v1_to_v2--> Main v2 --migrate_v2_to_v3--> Main v3 (current)
```

### 10.3 Migration Safety

1. **Auto-backup before migration** -- The original file is copied to `$DATA_DIR/hypercolor/backups/<filename>.<timestamp>.bak` before any modification.

2. **Atomic writes** -- Write to a `.tmp` file, then `rename()` (atomic on Linux ext4/btrfs; best-effort on Windows NTFS).

3. **Migration log** -- Every migration is recorded in `$STATE_DIR/hypercolor/migration.log`:
   ```
   2026-03-15T10:30:00Z  profiles/gaming.toml  v1 -> v2  "Add audio sensitivity settings per zone"
   2026-03-15T10:30:00Z  hypercolor.toml       v2 -> v3  "Add feature flags section"
   ```

4. **Dry-run mode** -- `hypercolor migrate --dry-run` shows what would change without modifying files.

5. **Format preservation** -- Migrations use `toml_edit::DocumentMut` (not `toml::Value`) to preserve comments, whitespace, and key ordering in user-edited files.

### 10.4 Backward Compatibility Guarantees

| Guarantee | Policy |
|---|---|
| Config files from older versions | Always auto-migrated on daemon start |
| Config files from newer versions | Rejected with clear error: "this config requires Hypercolor >= X.Y.Z" |
| Removed settings | Preserved as `[deprecated]` section for one major version, then stripped |
| Renamed settings | Migration renames them; old name works for one major version with deprecation warning |
| Breaking schema changes | Only in major versions (0.x -> 1.0, 1.x -> 2.0). Always with auto-migration |

### 10.5 Optional Version Compatibility Header

```toml
schema_version = 3
min_hypercolor_version = "0.3.0"     # Daemon rejects if its version is lower
```

---

## 11. Environment Variable Overrides

Every config key maps to an environment variable with `HYPERCOLOR_` prefix and `__` (double underscore) as the section separator.

### 11.1 Naming Convention

```
HYPERCOLOR_<SECTION>__<KEY>
```

Section and key names are UPPER_SNAKE_CASE. Nested sections use additional `__` separators.

### 11.2 Examples

| Environment Variable | Config Path | Type |
|---|---|---|
| `HYPERCOLOR_DAEMON__PORT` | `daemon.port` | `u16` |
| `HYPERCOLOR_DAEMON__TARGET_FPS` | `daemon.target_fps` | `u32` |
| `HYPERCOLOR_DAEMON__LOG_LEVEL` | `daemon.log_level` | `String` |
| `HYPERCOLOR_DAEMON__LISTEN_ADDRESS` | `daemon.listen_address` | `String` |
| `HYPERCOLOR_WEB__ENABLED` | `web.enabled` | `bool` |
| `HYPERCOLOR_WEB__WEBSOCKET_FPS` | `web.websocket_fps` | `u32` |
| `HYPERCOLOR_AUDIO__DEVICE` | `audio.device` | `String` |
| `HYPERCOLOR_AUDIO__FFT_SIZE` | `audio.fft_size` | `u32` |
| `HYPERCOLOR_AUDIO__ENABLED` | `audio.enabled` | `bool` |
| `HYPERCOLOR_CAPTURE__ENABLED` | `capture.enabled` | `bool` |
| `HYPERCOLOR_CAPTURE__MONITOR` | `capture.monitor` | `u32` |
| `HYPERCOLOR_DBUS__ENABLED` | `dbus.enabled` | `bool` |
| `HYPERCOLOR_TUI__THEME` | `tui.theme` | `String` |
| `HYPERCOLOR_FEATURES__WASM_PLUGINS` | `features.wasm_plugins` | `bool` |

### 11.3 Type Coercion

| Target Type | Accepted Values |
|---|---|
| `bool` | `true`, `false`, `1`, `0`, `yes`, `no` |
| `u16`, `u32`, `u64` | Decimal integers |
| `f32` | Decimal numbers with optional fractional part |
| `String` | Raw string value |
| `Vec<String>` | Comma-separated values: `"origin1,origin2"` |

### 11.4 Use Cases

Environment overrides are intended for:

- **systemd unit files** -- Override port or log level without editing config.
- **Container environments** -- Set device addresses for Docker/Podman deployments.
- **CI/testing** -- Run with specific settings without modifying the config tree.

```ini
# /etc/systemd/system/hypercolor.service.d/override.conf
[Service]
Environment=HYPERCOLOR_DAEMON__LISTEN_ADDRESS=0.0.0.0
Environment=HYPERCOLOR_DAEMON__LOG_LEVEL=debug
Environment=HYPERCOLOR_WEB__AUTH_ENABLED=true
```

### 11.5 Implementation

Environment variable resolution uses the `config` crate's layered approach:

```rust
use config::{Config, Environment, File};

pub fn load_config() -> Result<HypercolorConfig> {
    let config_path = config_dir().join("hypercolor.toml");

    let builder = Config::builder()
        // Layer 1: Compile-time defaults
        .set_default("daemon.port", 9420)?
        .set_default("daemon.target_fps", 60)?
        // ... all other defaults ...

        // Layer 2: System-wide config (Linux only)
        .add_source(
            File::with_name("/etc/hypercolor/hypercolor")
                .required(false)
        )

        // Layer 3: User config
        .add_source(
            File::from(config_path.clone())
                .required(false)
        )

        // Layer 4: Machine-local overrides
        .add_source(
            File::from(config_path.with_file_name("hypercolor.local.toml"))
                .required(false)
        )

        // Layer 5: Environment variables
        .add_source(
            Environment::with_prefix("HYPERCOLOR")
                .separator("__")
        )

        .build()?;

    config.try_deserialize()
}
```

---

## 12. Cross-Platform Paths

### 12.1 Path Summary

| Concept | Linux | Windows |
|---|---|---|
| **Config root** | `~/.config/hypercolor/` | `%APPDATA%\hypercolor\` |
| **Data root** | `~/.local/share/hypercolor/` | `%LOCALAPPDATA%\hypercolor\` |
| **State root** | `~/.local/state/hypercolor/` | `%LOCALAPPDATA%\hypercolor\state\` |
| **Cache root** | `~/.cache/hypercolor/` | `%LOCALAPPDATA%\hypercolor\cache\` |
| **Runtime** | `/run/user/$UID/hypercolor/` | N/A (named pipe) |
| **IPC socket** | `/run/user/$UID/hypercolor/hypercolor.sock` | `\\.\pipe\hypercolor` |
| **PID file** | `/run/user/$UID/hypercolor/hypercolor.pid` | N/A |
| **System defaults** | `/etc/hypercolor/` | N/A |

### 12.2 Platform-Specific Behavior

| Feature | Linux | Windows |
|---|---|---|
| IPC mechanism | Unix domain socket | Named pipe |
| D-Bus integration | Yes (`[dbus]` section) | Skipped |
| Screen capture | PipeWire, X11 | DXGI (Desktop Duplication API) |
| Audio capture | PulseAudio/PipeWire | WASAPI |
| File watcher | `inotify` via `notify` | `ReadDirectoryChangesW` via `notify` |
| Atomic rename | Guaranteed on ext4/btrfs | Best-effort on NTFS |
| Keyring | Secret Service D-Bus API | Windows Credential Manager |

### 12.3 Environment Variable Overrides for Paths

Default paths can be overridden for non-standard installations:

| Variable | Effect |
|---|---|
| `HYPERCOLOR_CONFIG_DIR` | Override config directory entirely |
| `HYPERCOLOR_DATA_DIR` | Override data directory entirely |
| `HYPERCOLOR_STATE_DIR` | Override state directory entirely |
| `HYPERCOLOR_CACHE_DIR` | Override cache directory entirely |
| `XDG_CONFIG_HOME` | Standard XDG override (Linux) |
| `XDG_DATA_HOME` | Standard XDG override (Linux) |
| `XDG_STATE_HOME` | Standard XDG override (Linux) |
| `XDG_CACHE_HOME` | Standard XDG override (Linux) |
| `XDG_RUNTIME_DIR` | Standard XDG override (Linux) |

`HYPERCOLOR_*_DIR` variables take precedence over `XDG_*` variables when both are set.

```rust
pub fn config_dir() -> PathBuf {
    // Explicit override takes precedence
    if let Ok(dir) = std::env::var("HYPERCOLOR_CONFIG_DIR") {
        return PathBuf::from(dir);
    }

    // Fall through to XDG / platform default
    #[cfg(target_os = "linux")]
    {
        std::env::var("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                dirs::home_dir().expect("HOME must be set").join(".config")
            })
            .join("hypercolor")
    }

    #[cfg(target_os = "windows")]
    {
        dirs::config_dir()
            .expect("APPDATA must be set")
            .join("hypercolor")
    }
}
```

### 12.4 First-Run Initialization

On first launch, the daemon creates the directory tree and writes a default `hypercolor.toml`:

```rust
pub fn ensure_directories() -> Result<()> {
    let dirs = [
        config_dir(),
        config_dir().join("devices"),
        config_dir().join("layouts"),
        config_dir().join("profiles"),
        config_dir().join("scenes"),
        config_dir().join("rules"),
        config_dir().join("schedules"),
        config_dir().join("templates"),
        data_dir(),
        data_dir().join("effects"),
        data_dir().join("effects/custom"),
        data_dir().join("effects/community"),
        data_dir().join("imports"),
        data_dir().join("backups"),
        state_dir(),
        cache_dir(),
    ];

    for dir in &dirs {
        std::fs::create_dir_all(dir)?;
    }

    // Write default config if none exists
    let config_path = config_dir().join("hypercolor.toml");
    if !config_path.exists() {
        let default = HypercolorConfig::default();
        let toml_str = toml::to_string_pretty(&default)?;
        std::fs::write(&config_path, toml_str)?;
    }

    Ok(())
}
```

### 12.5 Dotfiles Integration

The entire `$CONFIG_DIR/hypercolor/` tree is designed to live in a git repo and be symlinked into place. Secrets are stored in the system keyring, never in config files. Device configs contain hardware IDs and calibration data (not sensitive).

Recommended `.gitignore` for a Hypercolor config in a dotfiles repo:

```gitignore
# Machine-specific overrides (not shared across machines)
hypercolor.local.toml

# Runtime artifacts (should not appear here, but safety net)
*.pid
*.sock
*.shm

# Never commit (should not be here, but just in case)
.secrets
*.key
*.pem
```
