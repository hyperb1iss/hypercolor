# 15 -- CLI Command Specification

> Every `hypercolor` invocation is a spell cast into a Unix socket. Make it count.

---

## Table of Contents

1. [Overview](#1-overview)
2. [Global Flags & Environment](#2-global-flags--environment)
3. [Top-Level Clap Structure](#3-top-level-clap-structure)
4. [Exit Codes](#4-exit-codes)
5. [`hypercolor daemon`](#5-hypercolor-daemon)
6. [`hypercolor tui`](#6-hypercolor-tui)
7. [`hypercolor status`](#7-hypercolor-status)
8. [`hypercolor set`](#8-hypercolor-set)
9. [`hypercolor off`](#9-hypercolor-off)
10. [`hypercolor list`](#10-hypercolor-list)
11. [`hypercolor device`](#11-hypercolor-device)
12. [`hypercolor profile`](#12-hypercolor-profile)
13. [`hypercolor scene`](#13-hypercolor-scene)
14. [`hypercolor capture`](#14-hypercolor-capture)
15. [`hypercolor config`](#15-hypercolor-config)
16. [`hypercolor plugin`](#16-hypercolor-plugin)
17. [`hypercolor watch`](#17-hypercolor-watch)
18. [`hypercolor export`](#18-hypercolor-export)
19. [`hypercolor import`](#19-hypercolor-import)
20. [`hypercolor diagnose`](#20-hypercolor-diagnose)
21. [`hypercolor completion`](#21-hypercolor-completion)
22. [`hypercolor setup`](#22-hypercolor-setup)
23. [Output Design](#23-output-design)
24. [NO_COLOR Compliance](#24-no_color-compliance)

---

## 1. Overview

The `hypercolor` binary is the single entry point for all CLI and TUI interaction. It uses [clap](https://docs.rs/clap) derive-mode for type-safe argument parsing, communicates with the running daemon over a Unix socket (local) or REST API (remote), and produces styled human-readable output by default with `--json` for machine consumption.

**Binary name:** `hypercolor`

**Transport selection:**
- If `--host` is set or `HYPERCOLOR_HOST` is defined, connect via TCP/REST to `http://<host>/api/v1`.
- Otherwise, connect via Unix socket at `HYPERCOLOR_SOCKET` (default: `/run/hypercolor/hypercolor.sock`).
- If the socket does not exist and no `--host` is given, exit with code `2` and a helpful error message.

**Design contract:** Every command that mutates state returns both human output (confirming what changed) and a JSON representation (for scripting). Every command that reads state returns a styled table or summary, or raw JSON. No command is fire-and-forget -- the daemon always acknowledges.

---

## 2. Global Flags & Environment

### Global Flags

These flags are available on every subcommand via clap's `global = true`.

| Flag | Short | Type | Default | Description |
|------|-------|------|---------|-------------|
| `--host <HOST>` | | `Option<String>` | `None` | Remote daemon address (`host:port`). Overrides socket transport. |
| `--socket <PATH>` | | `Option<String>` | `/run/hypercolor/hypercolor.sock` | Unix socket path for local daemon communication. |
| `--json` | `-j` | `bool` | `false` | Output machine-readable JSON instead of styled text. |
| `--quiet` | `-q` | `bool` | `false` | Suppress all non-essential output. Only emit the core data. |
| `--no-color` | | `bool` | `false` | Disable ANSI color codes in output. |
| `--verbose` | `-v` | `bool` | `false` | Enable verbose/debug output. Repeatable (`-vv` for trace). |

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `HYPERCOLOR_SOCKET` | `/run/hypercolor/hypercolor.sock` | Unix socket path. Overridden by `--socket`. |
| `HYPERCOLOR_HOST` | *(none)* | Remote daemon address. Overridden by `--host`. |
| `HYPERCOLOR_CONFIG` | `~/.config/hypercolor/config.toml` | Configuration file path. |
| `HYPERCOLOR_COLOR` | `auto` | Color mode: `auto`, `always`, `never`. |
| `NO_COLOR` | *(none)* | When set to any value, disables color output ([no-color.org](https://no-color.org)). |
| `HYPERCOLOR_LOG` | `warn` | Log level for daemon startup: `error`, `warn`, `info`, `debug`, `trace`. |

### Color Resolution Order

Color output is determined by the first matching rule:

1. `--no-color` flag present --> colors disabled.
2. `NO_COLOR` env var set (any value) --> colors disabled.
3. `HYPERCOLOR_COLOR=never` --> colors disabled.
4. `HYPERCOLOR_COLOR=always` --> colors enabled (even when piped).
5. stdout is not a TTY --> colors disabled.
6. Otherwise --> colors enabled.

---

## 3. Top-Level Clap Structure

```rust
use clap::{Parser, Subcommand, ValueEnum, Args};
use std::path::PathBuf;

/// RGB lighting orchestration engine for Linux
#[derive(Parser)]
#[command(name = "hypercolor")]
#[command(version, about, long_about = None)]
#[command(propagate_version = true)]
#[command(styles = hypercolor_clap_styles())]
pub struct Cli {
    /// Remote daemon address (host:port). Uses REST transport.
    #[arg(long, global = true, env = "HYPERCOLOR_HOST")]
    pub host: Option<String>,

    /// Unix socket path for local daemon communication
    #[arg(long, global = true, env = "HYPERCOLOR_SOCKET")]
    pub socket: Option<PathBuf>,

    /// Output machine-readable JSON
    #[arg(long, short = 'j', global = true)]
    pub json: bool,

    /// Suppress non-essential output
    #[arg(long, short, global = true)]
    pub quiet: bool,

    /// Disable colored output
    #[arg(long, global = true, env = "NO_COLOR")]
    pub no_color: bool,

    /// Increase verbosity (-v info, -vv debug, -vvv trace)
    #[arg(long, short, global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Start, stop, or manage the Hypercolor daemon
    Daemon(DaemonArgs),
    /// Launch the interactive terminal UI
    Tui(TuiArgs),
    /// Show current system state
    Status(StatusArgs),
    /// Set the active lighting effect
    Set(SetArgs),
    /// Turn all LEDs off
    Off(OffArgs),
    /// List resources (devices, effects, profiles, scenes, layouts)
    List(ListArgs),
    /// Device discovery and management
    Device(DeviceArgs),
    /// Profile management (save, apply, delete)
    Profile(ProfileArgs),
    /// Scene management (automated lighting triggers)
    Scene(SceneArgs),
    /// Input capture control (audio, screen)
    Capture(CaptureArgs),
    /// Configuration management
    Config(ConfigArgs),
    /// Plugin management (install, remove, update)
    Plugin(PluginArgs),
    /// Stream live events and metrics
    Watch(WatchArgs),
    /// Export configuration and profiles for backup
    Export(ExportArgs),
    /// Import configuration and profiles from backup
    Import(ImportArgs),
    /// Run system diagnostics and health checks
    Diagnose(DiagnoseArgs),
    /// Generate shell completion scripts
    Completion(CompletionArgs),
    /// First-time setup wizard
    Setup(SetupArgs),
}
```

---

## 4. Exit Codes

All commands use consistent exit codes for scripting.

| Code | Name | Meaning |
|------|------|---------|
| `0` | `SUCCESS` | Command completed successfully. |
| `1` | `GENERAL_ERROR` | Unspecified error (catch-all). |
| `2` | `DAEMON_UNAVAILABLE` | Cannot connect to daemon (socket missing, connection refused). |
| `3` | `NOT_FOUND` | Requested resource does not exist (effect, device, profile). |
| `4` | `INVALID_INPUT` | Invalid arguments, parameter values, or option combinations. |
| `5` | `TIMEOUT` | Operation timed out (discovery, device test). |
| `6` | `CONFLICT` | State conflict (device already connected, profile name exists). |
| `7` | `PERMISSION_DENIED` | Insufficient permissions (network API key, socket perms). |
| `130` | `INTERRUPTED` | User interrupted with Ctrl+C (SIGINT). |

```rust
#[repr(u8)]
pub enum ExitCode {
    Success = 0,
    GeneralError = 1,
    DaemonUnavailable = 2,
    NotFound = 3,
    InvalidInput = 4,
    Timeout = 5,
    Conflict = 6,
    PermissionDenied = 7,
}
```

---

## 5. `hypercolor daemon`

Manage the daemon lifecycle. The daemon is the core process that drives the render loop, manages devices, and serves all API surfaces.

### Clap Structure

```rust
#[derive(Args)]
pub struct DaemonArgs {
    #[command(subcommand)]
    pub command: DaemonCommand,
}

#[derive(Subcommand)]
pub enum DaemonCommand {
    /// Start the daemon process
    Start(DaemonStartArgs),
    /// Gracefully stop the running daemon
    Stop,
    /// Stop and restart the daemon
    Restart(DaemonStartArgs),
    /// Show daemon health and status
    Status,
}

#[derive(Args)]
pub struct DaemonStartArgs {
    /// HTTP API listen port
    #[arg(long, default_value = "9420")]
    pub port: u16,

    /// Disable the embedded web UI
    #[arg(long)]
    pub no_web: bool,

    /// Log level for the daemon process
    #[arg(long, default_value = "warn", env = "HYPERCOLOR_LOG")]
    pub log_level: LogLevel,

    /// Network bind address (default: 127.0.0.1, local only)
    #[arg(long, default_value = "127.0.0.1")]
    pub bind: String,

    /// Run in foreground (don't daemonize)
    #[arg(long)]
    pub foreground: bool,

    /// Path to config file
    #[arg(long, env = "HYPERCOLOR_CONFIG")]
    pub config: Option<PathBuf>,

    /// Enable event logging to file
    #[arg(long)]
    pub event_log: Option<PathBuf>,
}

#[derive(ValueEnum, Clone)]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}
```

### Examples

```bash
# Start the daemon with defaults (localhost:9420, web UI enabled)
hypercolor daemon start

# Start on a custom port, no web UI, debug logging
hypercolor daemon start --port 8080 --no-web --log-level debug

# Start in foreground (for systemd or debugging)
hypercolor daemon start --foreground

# Start bound to all interfaces (enables network access, requires API key)
hypercolor daemon start --bind 0.0.0.0

# Check if daemon is running
hypercolor daemon status

# Graceful shutdown
hypercolor daemon stop

# Restart with same options
hypercolor daemon restart
```

### Output

**`hypercolor daemon status`** (human):

```
  Hypercolor Daemon
  Status     ● running          pid 4821
  Uptime     3h 42m
  Engine     wgpu (Vulkan 1.3)
  API        http://127.0.0.1:9420
  Socket     /run/hypercolor/hypercolor.sock
  Web UI     http://127.0.0.1:9420/
  Devices    4 connected (1,356 LEDs)
  Effect     Rainbow Wave
  FPS        60.0 actual / 60 target
```

**`hypercolor daemon status --json`**:

```json
{
  "status": "running",
  "pid": 4821,
  "uptime_seconds": 13320,
  "version": "0.1.0",
  "engine": {
    "type": "wgpu",
    "backend": "Vulkan 1.3"
  },
  "api": {
    "address": "127.0.0.1",
    "port": 9420,
    "web_ui": true,
    "socket": "/run/hypercolor/hypercolor.sock"
  },
  "devices": {
    "connected": 4,
    "total_leds": 1356
  },
  "effect": {
    "id": "rainbow-wave",
    "name": "Rainbow Wave"
  },
  "fps": {
    "target": 60,
    "actual": 60.0
  }
}
```

**`hypercolor daemon status --quiet`** exits `0` if running, `2` if not. No output.

### Exit Codes

| Subcommand | Success | Failure |
|------------|---------|---------|
| `start` | `0` -- daemon started | `1` -- already running or start failed |
| `stop` | `0` -- daemon stopped | `2` -- daemon not running |
| `restart` | `0` -- daemon restarted | `2` -- daemon not running / `1` -- restart failed |
| `status` | `0` -- daemon running | `2` -- daemon not running |

---

## 6. `hypercolor tui`

Launch the interactive terminal user interface. The TUI connects to the daemon and provides a full-screen Ratatui-based interface for real-time monitoring and control.

### Clap Structure

```rust
#[derive(Args)]
pub struct TuiArgs {
    /// Remote daemon address (overrides global --host)
    #[arg(long)]
    pub host: Option<String>,

    /// TUI color theme
    #[arg(long, default_value = "silkcircuit")]
    pub theme: String,

    /// TUI rendering frame rate (1-120)
    #[arg(long, default_value = "30")]
    pub fps: u32,

    /// Enable low-bandwidth mode (reduced updates for SSH/constrained connections)
    #[arg(long)]
    pub low_bandwidth: bool,

    /// Start on a specific view (dashboard, effects, control, devices, profiles, settings, debug)
    #[arg(long, default_value = "dashboard")]
    pub view: TuiView,
}

#[derive(ValueEnum, Clone)]
pub enum TuiView {
    Dashboard,
    Effects,
    Control,
    Devices,
    Profiles,
    Settings,
    Debug,
}
```

### Examples

```bash
# Launch TUI with defaults
hypercolor tui

# Connect to a remote daemon
hypercolor tui --host 192.168.1.42:9420

# Low-bandwidth mode for SSH over cellular
hypercolor tui --low-bandwidth

# Start directly on the effect browser at 60fps
hypercolor tui --view effects --fps 60
```

### Output

The TUI takes over the terminal alternate screen. No text output -- it renders a full-screen interactive interface. On exit (`q` / `Ctrl+C`), the terminal is restored to its previous state.

### Exit Codes

| Code | Meaning |
|------|---------|
| `0` | Clean exit (user quit) |
| `2` | Cannot connect to daemon |
| `1` | TUI crashed or terminal error |

---

## 7. `hypercolor status`

Display a snapshot of the current system state: running effect, connected devices, FPS, audio capture status, and active profile.

### Clap Structure

```rust
#[derive(Args)]
pub struct StatusArgs {
    /// Live-updating status (re-renders on state change)
    #[arg(long)]
    pub watch: bool,

    /// Update interval for --watch mode in seconds
    #[arg(long, default_value = "1")]
    pub interval: f64,
}
```

### Examples

```bash
# One-shot status
hypercolor status

# Machine-readable
hypercolor status --json

# Minimal (just the effect name)
hypercolor status --quiet

# Live-updating status (like watch(1))
hypercolor status --watch

# Pipe to jq
hypercolor status --json | jq '.effect.name'
```

### Output

**Human:**

```
  ╭─ Hypercolor ────────────────────────────────────────────────╮
  │                                                              │
  │   Daemon     ● running          pid 4821   uptime 3h 42m    │
  │   Engine     wgpu (Vulkan 1.3)  60.0 fps   16.4ms/frame    │
  │   Audio      PipeWire monitor   48kHz      ● capturing      │
  │   Effect     Rainbow Wave       speed: 65  palette: Aurora  │
  │   Profile    Evening            since 20:18                  │
  │                                                              │
  │   ── Devices ────────────────────────────────────────────   │
  │                                                              │
  │   WLED Living Room       ● 120 LEDs    DDP     0.8ms        │
  │   Prism 8 Controller     ● 1008 LEDs   HID     2.1ms        │
  │   Strimer ATX            ● 120 LEDs    HID     1.8ms        │
  │   Strimer GPU            ● 108 LEDs    HID     1.8ms        │
  │                                                              │
  │   Total: 1,356 LEDs across 4 devices                        │
  │                                                              │
  ╰──────────────────────────────────────────────────────────────╯
```

**JSON:**

```json
{
  "daemon": {
    "status": "running",
    "pid": 4821,
    "uptime_seconds": 13320
  },
  "engine": {
    "type": "wgpu",
    "backend": "Vulkan 1.3",
    "fps": 60.0,
    "frame_time_ms": 16.4
  },
  "audio": {
    "source": "PipeWire monitor",
    "sample_rate": 48000,
    "capturing": true,
    "level": 0.58,
    "beat": false
  },
  "effect": {
    "id": "rainbow-wave",
    "name": "Rainbow Wave",
    "controls": {
      "speed": 65,
      "intensity": 85,
      "palette": "Aurora",
      "direction": "right"
    }
  },
  "profile": {
    "id": "evening",
    "name": "Evening",
    "applied_at": "2026-03-01T20:18:00Z"
  },
  "devices": [
    {
      "name": "WLED Living Room",
      "protocol": "ddp",
      "leds": 120,
      "status": "connected",
      "latency_ms": 0.8
    },
    {
      "name": "Prism 8 Controller",
      "protocol": "hid",
      "leds": 1008,
      "status": "connected",
      "latency_ms": 2.1
    },
    {
      "name": "Strimer ATX",
      "protocol": "hid",
      "leds": 120,
      "status": "connected",
      "latency_ms": 1.8
    },
    {
      "name": "Strimer GPU",
      "protocol": "hid",
      "leds": 108,
      "status": "connected",
      "latency_ms": 1.8
    }
  ],
  "total_leds": 1356
}
```

**Quiet:** Prints only the current effect name, or `Off` if paused.

```
Rainbow Wave
```

### Exit Codes

| Code | Meaning |
|------|---------|
| `0` | Status retrieved |
| `2` | Daemon not running |

---

## 8. `hypercolor set`

Set the active lighting effect. Accepts the effect name (or slug) as a positional argument, with optional parameter overrides. The daemon fuzzy-matches the name against all available effects.

### Clap Structure

```rust
#[derive(Args)]
pub struct SetArgs {
    /// Effect name or slug (fuzzy-matched)
    pub effect: String,

    /// Target specific device(s) by name or ID (repeatable)
    #[arg(long, short)]
    pub device: Vec<String>,

    /// Target specific zone(s) within a device
    #[arg(long, short = 'z')]
    pub zone: Vec<String>,

    /// Set arbitrary control parameters (repeatable, format: key=value)
    #[arg(long, short, value_parser = parse_key_value)]
    pub param: Vec<(String, String)>,

    /// Speed control shorthand (0-100)
    #[arg(long)]
    pub speed: Option<u32>,

    /// Intensity control shorthand (0-100)
    #[arg(long)]
    pub intensity: Option<u32>,

    /// Color for solid/color-based effects (hex: #ff00ff or name: cyan)
    #[arg(long)]
    pub color: Option<String>,

    /// Palette name for palette-based effects
    #[arg(long)]
    pub palette: Option<String>,

    /// Crossfade transition duration in milliseconds
    #[arg(long, default_value = "500")]
    pub transition: u32,

    /// Set brightness (0-100) along with the effect
    #[arg(long)]
    pub brightness: Option<u8>,
}

fn parse_key_value(s: &str) -> Result<(String, String), String> {
    let pos = s.find('=')
        .ok_or_else(|| format!("invalid KEY=VALUE: no '=' found in '{s}'"))?;
    Ok((s[..pos].to_string(), s[pos + 1..].to_string()))
}
```

### Examples

```bash
# Set effect by name (fuzzy-matched)
hypercolor set rainbow-wave

# Set with speed shorthand
hypercolor set rainbow-wave --speed 90

# Set with multiple parameters
hypercolor set aurora-drift --speed 45 --palette ocean --intensity 60

# Set solid color
hypercolor set solid-color --color "#ff6ac1"

# Set with arbitrary params via key=value
hypercolor set plasma-storm --param turbulence=80 --param scale=2.5

# Target a specific device
hypercolor set breathing --device "WLED Living Room" --color "#80ffea"

# Slow crossfade transition
hypercolor set aurora-drift --transition 3000

# Set effect and brightness together
hypercolor set rainbow-wave --speed 80 --brightness 60
```

### Output

**Human:**

```
  ✦ Effect set: Rainbow Wave
    speed: 90  intensity: 85  palette: Aurora  direction: right
    Applied to 4 devices (1,356 LEDs)
```

**Human (with device targeting):**

```
  ✦ Effect set: Breathing
    color: #80ffea
    Applied to WLED Living Room (120 LEDs)
```

**Human (fuzzy match with alternatives):**

```
  ✦ Effect set: Aurora Drift (matched from "aurora")
    speed: 45  palette: Ocean
    Applied to 4 devices (1,356 LEDs)
    Also matched: Aurora Borealis, Aurora Shimmer
```

**JSON:**

```json
{
  "effect": {
    "id": "rainbow-wave",
    "name": "Rainbow Wave",
    "match_confidence": 1.0
  },
  "controls": {
    "speed": 90,
    "intensity": 85,
    "palette": "Aurora",
    "direction": "right"
  },
  "devices": [
    { "name": "WLED Living Room", "leds": 120 },
    { "name": "Prism 8 Controller", "leds": 1008 },
    { "name": "Strimer ATX", "leds": 120 },
    { "name": "Strimer GPU", "leds": 108 }
  ],
  "total_leds": 1356,
  "transition_ms": 500
}
```

**Quiet:** No output on success.

### Error Output

```
  ✗ Effect not found: "rainbo"
    Did you mean: Rainbow Wave, Rainbow Spiral
    Use 'hypercolor list effects' to see all available effects.
```

```
  ✗ Invalid value for --speed: "fast"
    Expected: number between 0 and 100
    Hint: --speed 90 for fast, --speed 30 for slow
```

### Exit Codes

| Code | Meaning |
|------|---------|
| `0` | Effect applied |
| `2` | Daemon not running |
| `3` | Effect not found (no fuzzy match above threshold) |
| `4` | Invalid parameter value |

---

## 9. `hypercolor off`

Turn off all LEDs. This pauses the render loop and sends black frames to all devices (or specific devices if targeted). The daemon stays running -- this is not `daemon stop`.

### Clap Structure

```rust
#[derive(Args)]
pub struct OffArgs {
    /// Turn off specific device(s) only (repeatable)
    #[arg(long, short)]
    pub device: Vec<String>,

    /// Fade-out transition duration in milliseconds (0 = instant)
    #[arg(long, default_value = "300")]
    pub transition: u32,
}
```

### Examples

```bash
# Turn everything off
hypercolor off

# Turn off a specific device
hypercolor off --device "WLED Living Room"

# Instant off (no fade)
hypercolor off --transition 0
```

### Output

**Human:**

```
  ○ All devices off (1,356 LEDs)
```

**Human (targeted):**

```
  ○ WLED Living Room off (120 LEDs)
    3 devices still active (1,236 LEDs)
```

**JSON:**

```json
{
  "action": "off",
  "devices_off": ["WLED Living Room", "Prism 8 Controller", "Strimer ATX", "Strimer GPU"],
  "leds_off": 1356,
  "transition_ms": 300
}
```

**Quiet:** No output.

### Exit Codes

| Code | Meaning |
|------|---------|
| `0` | Success |
| `2` | Daemon not running |
| `3` | Named device not found |

---

## 10. `hypercolor list`

List resources by category. Clean tabular output with optional filtering.

### Clap Structure

```rust
#[derive(Args)]
pub struct ListArgs {
    #[command(subcommand)]
    pub resource: ListResource,
}

#[derive(Subcommand)]
pub enum ListResource {
    /// List connected and discovered devices
    Devices(ListDevicesArgs),
    /// List available lighting effects
    Effects(ListEffectsArgs),
    /// List saved profiles
    Profiles,
    /// List configured scenes
    Scenes,
    /// List spatial layouts
    Layouts,
}

#[derive(Args)]
pub struct ListDevicesArgs {
    /// Filter by connection status
    #[arg(long)]
    pub status: Option<DeviceStatusFilter>,

    /// Filter by backend/protocol
    #[arg(long)]
    pub backend: Option<String>,
}

#[derive(ValueEnum, Clone)]
pub enum DeviceStatusFilter {
    Connected,
    Disconnected,
    Discovered,
    All,
}

#[derive(Args)]
pub struct ListEffectsArgs {
    /// Filter by engine type
    #[arg(long)]
    pub engine: Option<EffectEngineFilter>,

    /// Filter to audio-reactive effects only
    #[arg(long)]
    pub audio: bool,

    /// Search effects by name or description
    #[arg(long)]
    pub search: Option<String>,

    /// Filter by category
    #[arg(long)]
    pub category: Option<String>,
}

#[derive(ValueEnum, Clone)]
pub enum EffectEngineFilter {
    Native,
    Web,
    Wasm,
    All,
}
```

### Examples

```bash
# List all devices
hypercolor list devices

# Only connected devices
hypercolor list devices --status connected

# List effects
hypercolor list effects

# Audio-reactive effects only
hypercolor list effects --audio

# Search effects
hypercolor list effects --search "aurora"

# Native effects only
hypercolor list effects --engine native

# List profiles
hypercolor list profiles

# List scenes
hypercolor list scenes

# List layouts
hypercolor list layouts

# JSON output for scripting
hypercolor list devices --json

# Quiet: just names, one per line
hypercolor list devices --quiet
```

### Output

**`hypercolor list devices` (human):**

```
  Device                     Protocol    LEDs    Status    Latency    Zone
  ──────────────────────────────────────────────────────────────────────────
  WLED Living Room           DDP          120    ● ok       0.8ms     Strip
  Prism 8 Controller         USB HID     1008    ● ok       2.1ms     8x Strip
  Strimer ATX (Prism S #1)   USB HID      120    ● ok       1.8ms     20x6 Matrix
  Strimer GPU (Prism S #1)   USB HID      108    ● ok       1.8ms     27x4 Matrix

  4 devices · 1,356 LEDs
```

**`hypercolor list effects` (human):**

```
  Effect                     Engine     Audio    Params    Author
  ──────────────────────────────────────────────────────────────────────────
  Rainbow Wave               ✦ native   no       4         Hypercolor
  Aurora Drift               ✦ native   yes      6         Hypercolor
  Plasma Storm               ✦ native   yes      5         Hypercolor
  Breathing                  ✦ native   no       3         Hypercolor
  Solid Color                ✦ native   no       1         Hypercolor
  Fire                       ✦ native   yes      4         Hypercolor
  Neon Highway               ◈ web      yes      7         Community
  Cosmic Pulse               ◈ web      yes      5         Community
  Digital Rain               ◈ web      no       3         Community

  9 effects (6 native, 3 web)
```

**`hypercolor list profiles` (human):**

```
  Profile         Effect          Active    Last Applied
  ──────────────────────────────────────────────────────────
  Evening         Aurora Drift    ● yes     today 20:18
  Gaming          Audio Pulse               yesterday 19:30
  Movie Night     Breathing                 2026-02-28
  Focus           Solid Color               2026-02-27
  Party Mode      Neon Highway              2026-02-25
  Sleep           Breathing                 today 23:00
  All Off         (off)                     2026-02-20

  7 profiles
```

**`hypercolor list scenes` (human):**

```
  Scene             Profile         Trigger         Enabled    Last Triggered
  ─────────────────────────────────────────────────────────────────────────────
  Sunset Warmth     Warm Ambient    sunset -15m     ● yes      today 17:45
  Bedtime           Night Mode      22:30 daily     ● yes      yesterday 22:30
  Gaming Hour       Gaming Mode     device: GPU     ○ no       —

  3 scenes
```

**JSON (`hypercolor list devices --json`):**

```json
[
  {
    "id": "wled_living_room_strip",
    "name": "WLED Living Room",
    "backend": "wled",
    "protocol": "ddp",
    "leds": 120,
    "status": "connected",
    "latency_ms": 0.8,
    "zone": "strip"
  }
]
```

**Quiet (`hypercolor list devices --quiet`):**

```
WLED Living Room
Prism 8 Controller
Strimer ATX
Strimer GPU
```

### Exit Codes

| Code | Meaning |
|------|---------|
| `0` | Success (even if list is empty) |
| `2` | Daemon not running |

---

## 11. `hypercolor device`

Device discovery, inspection, testing, and management.

### Clap Structure

```rust
#[derive(Args)]
pub struct DeviceArgs {
    #[command(subcommand)]
    pub command: DeviceCommand,
}

#[derive(Subcommand)]
pub enum DeviceCommand {
    /// Scan for new RGB devices across all backends
    Discover(DeviceDiscoverArgs),
    /// Flash a test pattern on a device for identification
    Identify(DeviceIdentifyArgs),
    /// Run a full diagnostic test pattern sequence
    Test(DeviceTestArgs),
    /// Show detailed information about a device
    Info(DeviceInfoArgs),
    /// Run device calibration (color order, LED count verification)
    Calibrate(DeviceCalibrateArgs),
    /// Connect to a discovered device
    Connect(DeviceConnectArgs),
    /// Disconnect a device
    Disconnect(DeviceDisconnectArgs),
    /// Rename a device
    Rename(DeviceRenameArgs),
}

#[derive(Args)]
pub struct DeviceDiscoverArgs {
    /// Scan specific backends only (repeatable: wled, hid, hue)
    #[arg(long)]
    pub backend: Vec<String>,

    /// Discovery timeout in seconds
    #[arg(long, default_value = "10")]
    pub timeout: u32,
}

#[derive(Args)]
pub struct DeviceIdentifyArgs {
    /// Device name or ID
    pub device: String,

    /// Flash duration in seconds
    #[arg(long, default_value = "5")]
    pub duration: u32,
}

#[derive(Args)]
pub struct DeviceTestArgs {
    /// Device name or ID
    pub device: String,
}

#[derive(Args)]
pub struct DeviceInfoArgs {
    /// Device name or ID
    pub device: String,
}

#[derive(Args)]
pub struct DeviceCalibrateArgs {
    /// Device name or ID
    pub device: String,
}

#[derive(Args)]
pub struct DeviceConnectArgs {
    /// Device name or ID
    pub device: String,
}

#[derive(Args)]
pub struct DeviceDisconnectArgs {
    /// Device name or ID
    pub device: String,
}

#[derive(Args)]
pub struct DeviceRenameArgs {
    /// Device name or ID
    pub device: String,

    /// New display name
    pub name: String,
}
```

### Examples

```bash
# Discover all devices
hypercolor device discover

# Discover WLED devices only, with 30s timeout
hypercolor device discover --backend wled --timeout 30

# Flash a device to identify which one it is
hypercolor device identify "WLED Living Room"

# Run test pattern sequence
hypercolor device test "Prism 8 Controller"

# Show detailed device info
hypercolor device info "WLED Living Room"

# Calibrate color order
hypercolor device calibrate "Strimer ATX"

# Connect to a discovered device
hypercolor device connect "WLED Kitchen"

# Disconnect a device
hypercolor device disconnect "WLED Living Room"

# Rename a device
hypercolor device rename "WLED Living Room" "Living Room Strip"
```

### Output

**`hypercolor device discover` (human):**

```
  Discovering devices...
  ⠸ Scanning USB HID...         found 3
  ⠴ Scanning mDNS (WLED)...     found 2
  ⠦ Scanning WLED...  timeout

  5 devices found (3 USB, 2 network)

  New:
    WLED Kitchen             WLED    192.168.1.43    60 LEDs
    WLED Desk                WLED    192.168.1.44    30 LEDs

  Already known:
    Prism 8 Controller       HID     USB 1-3.2       1008 LEDs
    Strimer ATX              HID     USB 1-3.3       120 LEDs
    Strimer GPU              HID     USB 1-3.4       108 LEDs
```

**`hypercolor device info "WLED Living Room"` (human):**

```
  WLED Living Room

  Protocol     WLED / DDP
  Address      192.168.1.42:4048
  LED Count    120
  Topology     Strip (linear)
  Color Fmt    GRB
  Output FPS   60
  Latency      0.8ms
  Firmware     WLED 0.15.3
  MAC          AA:BB:CC:DD:EE:FF
  Last Seen    2026-03-01 12:00:00

  Zone Mapping:
    Canvas position    (0.1, 0.3)
    Canvas size        (0.8, 0.05)
    Rotation           0deg
    Mirror             no
    Reverse            no
```

**`hypercolor device test "WLED Living Room"` (human):**

```
  Testing WLED Living Room...
  ▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓░░░░░░░░░░  Phase 3/5: Blue sweep
  Test complete. Device responding normally.
```

**JSON (`hypercolor device info --json`):**

```json
{
  "id": "wled_living_room_strip",
  "name": "WLED Living Room",
  "backend": "wled",
  "protocol": "ddp",
  "address": "192.168.1.42",
  "port": 4048,
  "led_count": 120,
  "topology": "strip",
  "color_order": "grb",
  "fps": 60,
  "latency_ms": 0.8,
  "firmware_version": "0.15.3",
  "mac_address": "AA:BB:CC:DD:EE:FF",
  "status": "connected",
  "last_seen": "2026-03-01T12:00:00Z",
  "zone_mapping": {
    "position": { "x": 0.1, "y": 0.3 },
    "size": { "w": 0.8, "h": 0.05 },
    "rotation": 0.0,
    "mirror": false,
    "reverse": false
  }
}
```

### Exit Codes

| Subcommand | `0` | Other |
|------------|-----|-------|
| `discover` | Scan complete (even if 0 found) | `2` daemon unavailable, `5` all backends timed out |
| `identify` | Flash started | `3` device not found |
| `test` | Test passed | `3` device not found, `1` test failed |
| `info` | Info retrieved | `3` device not found |
| `calibrate` | Calibration complete | `3` device not found |
| `connect` | Connected | `3` device not found, `6` already connected |
| `disconnect` | Disconnected | `3` device not found |
| `rename` | Renamed | `3` device not found |

---

## 12. `hypercolor profile`

Manage lighting profiles -- saved snapshots of effect + controls + device configuration.

### Clap Structure

```rust
#[derive(Args)]
pub struct ProfileArgs {
    #[command(subcommand)]
    pub command: ProfileCommand,
}

#[derive(Subcommand)]
pub enum ProfileCommand {
    /// Apply a saved profile
    Apply(ProfileApplyArgs),
    /// Save the current state as a new profile
    Save(ProfileSaveArgs),
    /// Delete a profile
    Delete(ProfileDeleteArgs),
    /// Export a profile to a TOML file
    Export(ProfileExportArgs),
    /// Import a profile from a TOML file
    Import(ProfileImportArgs),
    /// List all profiles (alias for 'list profiles')
    List,
    /// Show detailed profile contents
    Show(ProfileShowArgs),
    /// Open a profile in $EDITOR for manual editing
    Edit(ProfileEditArgs),
}

#[derive(Args)]
pub struct ProfileApplyArgs {
    /// Profile name or ID (fuzzy-matched)
    pub name: String,

    /// Crossfade transition duration in milliseconds
    #[arg(long, default_value = "500")]
    pub transition: u32,
}

#[derive(Args)]
pub struct ProfileSaveArgs {
    /// Profile name
    pub name: String,

    /// Profile description
    #[arg(long)]
    pub description: Option<String>,

    /// Overwrite if profile already exists
    #[arg(long)]
    pub force: bool,
}

#[derive(Args)]
pub struct ProfileDeleteArgs {
    /// Profile name or ID
    pub name: String,

    /// Skip confirmation prompt
    #[arg(long)]
    pub yes: bool,
}

#[derive(Args)]
pub struct ProfileExportArgs {
    /// Profile name or ID
    pub name: String,

    /// Output file path (default: <name>.toml)
    #[arg(long, short)]
    pub output: Option<PathBuf>,
}

#[derive(Args)]
pub struct ProfileImportArgs {
    /// TOML file to import
    pub file: PathBuf,

    /// Override profile name from file
    #[arg(long)]
    pub name: Option<String>,
}

#[derive(Args)]
pub struct ProfileShowArgs {
    /// Profile name or ID
    pub name: String,
}

#[derive(Args)]
pub struct ProfileEditArgs {
    /// Profile name or ID
    pub name: String,
}
```

### Examples

```bash
# Apply a profile
hypercolor profile apply evening

# Apply with slow crossfade
hypercolor profile apply "gaming" --transition 3000

# Save current state
hypercolor profile save "late-night" --description "Dim aurora for late coding"

# Save, overwriting existing
hypercolor profile save "evening" --force

# Show profile detail
hypercolor profile show evening

# Delete a profile
hypercolor profile delete "old-profile"

# Delete without confirmation
hypercolor profile delete "old-profile" --yes

# Export to file
hypercolor profile export evening --output ~/evening.toml

# Import from file
hypercolor profile import ~/shared-profile.toml

# Open in $EDITOR
hypercolor profile edit evening

# List all profiles
hypercolor profile list
```

### Output

**`hypercolor profile apply evening` (human):**

```
  ✦ Profile: Evening
    Effect      Aurora Drift (speed: 45, palette: Ocean)
    Brightness  WLED: 40%  Strimers: 60%  Prism 8 Ch5-8: off
    Audio       disabled
    Applied in 120ms
```

**`hypercolor profile show evening` (human):**

```
  Profile: Evening

  Effect        Aurora Drift
  Controls      speed=45  intensity=60  palette=Ocean
  Audio         disabled

  Device Overrides:
    WLED Living Room       brightness: 40%
    Strimer ATX            brightness: 60%
    Strimer GPU            brightness: 60%
    Prism 8 Ch5-8          off

  Schedule:
    Activate at            sunset
    Deactivate at          23:00

  Created       2026-02-15
  Last applied  today 20:18
```

**`hypercolor profile save "late-night"` (human):**

```
  ✦ Profile saved: late-night
    Effect      Aurora Drift (speed: 30, palette: Midnight)
    Devices     4 active
    Brightness  global: 25%
```

**JSON (`hypercolor profile apply evening --json`):**

```json
{
  "profile": {
    "id": "evening",
    "name": "Evening",
    "description": "Warm evening lighting"
  },
  "effect": {
    "id": "aurora-drift",
    "name": "Aurora Drift",
    "controls": {
      "speed": 45,
      "intensity": 60,
      "palette": "Ocean"
    }
  },
  "devices": {
    "wled_living_room_strip": { "enabled": true, "brightness": 40 },
    "strimer_atx": { "enabled": true, "brightness": 60 },
    "strimer_gpu": { "enabled": true, "brightness": 60 },
    "prism8_ch5": { "enabled": false },
    "prism8_ch6": { "enabled": false },
    "prism8_ch7": { "enabled": false },
    "prism8_ch8": { "enabled": false }
  },
  "applied_ms": 120
}
```

### Exit Codes

| Subcommand | `0` | Other |
|------------|-----|-------|
| `apply` | Applied | `3` profile not found |
| `save` | Saved | `6` name exists (without `--force`) |
| `delete` | Deleted | `3` not found |
| `export` | Exported | `3` not found, `1` write error |
| `import` | Imported | `4` invalid file, `6` name exists |
| `show` | Displayed | `3` not found |
| `edit` | Saved after edit | `3` not found, `4` invalid after edit |
| `list` | Listed | `2` daemon unavailable |

---

## 13. `hypercolor scene`

Manage automated lighting scenes -- profiles that activate based on triggers (time, solar events, device state, etc.).

### Clap Structure

```rust
#[derive(Args)]
pub struct SceneArgs {
    #[command(subcommand)]
    pub command: SceneCommand,
}

#[derive(Subcommand)]
pub enum SceneCommand {
    /// Manually activate a scene (trigger its profile)
    Activate(SceneActivateArgs),
    /// Create a new scene
    Create(SceneCreateArgs),
    /// Delete a scene
    Delete(SceneDeleteArgs),
    /// List all scenes (alias for 'list scenes')
    List,
    /// Show detailed scene configuration
    Show(SceneShowArgs),
    /// Enable or disable a scene
    Enable(SceneEnableArgs),
    /// Disable a scene
    Disable(SceneDisableArgs),
}

#[derive(Args)]
pub struct SceneActivateArgs {
    /// Scene name or ID
    pub name: String,

    /// Override transition duration (ms)
    #[arg(long)]
    pub transition: Option<u32>,
}

#[derive(Args)]
pub struct SceneCreateArgs {
    /// Scene name
    pub name: String,

    /// Profile to activate when triggered
    #[arg(long, required = true)]
    pub profile: String,

    /// Trigger type: schedule, sunset, sunrise, device, audio
    #[arg(long, required = true)]
    pub trigger: String,

    /// Cron expression (for schedule trigger)
    #[arg(long)]
    pub cron: Option<String>,

    /// Offset in minutes from solar event (for sunset/sunrise)
    #[arg(long, default_value = "0")]
    pub offset: i32,

    /// Transition duration in milliseconds
    #[arg(long, default_value = "1000")]
    pub transition: u32,

    /// Start enabled
    #[arg(long, default_value = "true")]
    pub enabled: bool,
}

#[derive(Args)]
pub struct SceneDeleteArgs {
    /// Scene name or ID
    pub name: String,

    /// Skip confirmation
    #[arg(long)]
    pub yes: bool,
}

#[derive(Args)]
pub struct SceneShowArgs {
    /// Scene name or ID
    pub name: String,
}

#[derive(Args)]
pub struct SceneEnableArgs {
    /// Scene name or ID
    pub name: String,
}

#[derive(Args)]
pub struct SceneDisableArgs {
    /// Scene name or ID
    pub name: String,
}
```

### Examples

```bash
# Manually fire a scene
hypercolor scene activate "sunset-warmth"

# Create a sunset-triggered scene
hypercolor scene create "sunset-warmth" \
  --profile "warm-ambient" \
  --trigger sunset \
  --offset -15 \
  --transition 3000

# Create a cron-scheduled scene
hypercolor scene create "bedtime" \
  --profile "night-mode" \
  --trigger schedule \
  --cron "30 22 * * *"

# List scenes
hypercolor scene list

# Show scene details
hypercolor scene show "sunset-warmth"

# Disable a scene
hypercolor scene disable "bedtime"

# Re-enable
hypercolor scene enable "bedtime"

# Delete a scene
hypercolor scene delete "old-scene" --yes
```

### Output

**`hypercolor scene activate "sunset-warmth"` (human):**

```
  ✦ Scene triggered: Sunset Warmth
    Profile: Warm Ambient
    Transition: 3000ms crossfade
```

**`hypercolor scene show "sunset-warmth"` (human):**

```
  Scene: Sunset Warmth

  Profile        Warm Ambient
  Trigger        sunset -15m
  Transition     3000ms crossfade
  Enabled        yes
  Last Triggered today 17:45
  Created        2026-02-20
```

**JSON:**

```json
{
  "id": "sunset_warmth",
  "name": "Sunset Warmth",
  "enabled": true,
  "profile_id": "warm_ambient",
  "trigger": {
    "type": "solar",
    "event": "sunset",
    "offset_minutes": -15
  },
  "transition": {
    "type": "crossfade",
    "duration_ms": 3000
  },
  "last_triggered": "2026-03-01T17:45:00Z",
  "created_at": "2026-02-20T10:00:00Z"
}
```

### Exit Codes

| Subcommand | `0` | Other |
|------------|-----|-------|
| `activate` | Triggered | `3` not found |
| `create` | Created | `6` name exists, `4` invalid trigger config |
| `delete` | Deleted | `3` not found |
| `show` | Displayed | `3` not found |
| `enable` | Enabled | `3` not found |
| `disable` | Disabled | `3` not found |
| `list` | Listed | `2` daemon unavailable |

---

## 14. `hypercolor capture`

Control audio and screen capture input sources.

### Clap Structure

```rust
#[derive(Args)]
pub struct CaptureArgs {
    #[command(subcommand)]
    pub source: CaptureSource,
}

#[derive(Subcommand)]
pub enum CaptureSource {
    /// Audio capture control
    Audio(CaptureAudioArgs),
    /// Screen capture control
    Screen(CaptureScreenArgs),
}

#[derive(Args)]
pub struct CaptureAudioArgs {
    #[command(subcommand)]
    pub command: CaptureAudioCommand,
}

#[derive(Subcommand)]
pub enum CaptureAudioCommand {
    /// Start audio capture
    Start(AudioStartArgs),
    /// Stop audio capture
    Stop,
    /// Show audio capture status and current levels
    Status,
}

#[derive(Args)]
pub struct AudioStartArgs {
    /// Audio source name (default: system default)
    #[arg(long)]
    pub source: Option<String>,

    /// Audio gain multiplier (0.0-5.0)
    #[arg(long)]
    pub gain: Option<f32>,

    /// Beat detection sensitivity (0.0-1.0)
    #[arg(long)]
    pub beat_sensitivity: Option<f32>,
}

#[derive(Args)]
pub struct CaptureScreenArgs {
    #[command(subcommand)]
    pub command: CaptureScreenCommand,
}

#[derive(Subcommand)]
pub enum CaptureScreenCommand {
    /// Start screen capture
    Start(ScreenStartArgs),
    /// Stop screen capture
    Stop,
    /// Show screen capture status
    Status,
}

#[derive(Args)]
pub struct ScreenStartArgs {
    /// Display index to capture (default: primary)
    #[arg(long)]
    pub display: Option<u32>,

    /// Capture frame rate (default: 30)
    #[arg(long, default_value = "30")]
    pub fps: u32,

    /// Capture region (format: WxH+X+Y, e.g., 1920x1080+0+0)
    #[arg(long)]
    pub region: Option<String>,
}
```

### Examples

```bash
# Start audio capture with default source
hypercolor capture audio start

# Start audio capture with specific source and gain
hypercolor capture audio start --source "PipeWire Monitor" --gain 1.5

# Check audio status
hypercolor capture audio status

# Stop audio
hypercolor capture audio stop

# Start screen capture
hypercolor capture screen start

# Start screen capture on display 2 at 15fps
hypercolor capture screen start --display 2 --fps 15

# Screen capture status
hypercolor capture screen status

# Stop screen capture
hypercolor capture screen stop
```

### Output

**`hypercolor capture audio status` (human):**

```
  Audio Capture

  Status       ● active
  Source       PipeWire Multimedia (default)
  Sample Rate  48000 Hz
  FFT Bins     200
  Gain         1.0x
  Beat Sens    0.6

  Current:
    Level     ▇▇▇▇▇▇░░ 58%
    Bass      ▇▇▇▇▇▇▇░ 71%
    Mid       ▇▇▇░░░░░ 35%
    Treble    ▇▇░░░░░░ 18%
    Beat      no (confidence: 0.45)
```

**JSON:**

```json
{
  "status": "active",
  "source": "PipeWire Multimedia (default)",
  "sample_rate": 48000,
  "fft_bins": 200,
  "gain": 1.0,
  "beat_sensitivity": 0.6,
  "current": {
    "level": 0.58,
    "bass": 0.71,
    "mid": 0.35,
    "treble": 0.18,
    "beat": false,
    "beat_confidence": 0.45
  }
}
```

### Exit Codes

| Subcommand | `0` | Other |
|------------|-----|-------|
| `audio start` | Started | `6` already active, `3` source not found |
| `audio stop` | Stopped | `1` not active |
| `audio status` | Reported | `2` daemon unavailable |
| `screen start` | Started | `6` already active, `3` display not found |
| `screen stop` | Stopped | `1` not active |
| `screen status` | Reported | `2` daemon unavailable |

---

## 15. `hypercolor config`

Read and write daemon/CLI configuration values. The config file is TOML at `~/.config/hypercolor/config.toml`.

### Clap Structure

```rust
#[derive(Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub command: ConfigCommand,
}

#[derive(Subcommand)]
pub enum ConfigCommand {
    /// Get a config value by dotted key path
    Get(ConfigGetArgs),
    /// Set a config value
    Set(ConfigSetArgs),
    /// Reset config to defaults (or a specific key)
    Reset(ConfigResetArgs),
    /// Open config file in $EDITOR
    Edit,
    /// Show the complete current configuration
    Show,
}

#[derive(Args)]
pub struct ConfigGetArgs {
    /// Dotted key path (e.g., daemon.fps, tui.refresh_fps, audio.gain)
    pub key: String,
}

#[derive(Args)]
pub struct ConfigSetArgs {
    /// Dotted key path
    pub key: String,

    /// New value
    pub value: String,

    /// Apply change to running daemon immediately (hot-reload)
    #[arg(long)]
    pub live: bool,
}

#[derive(Args)]
pub struct ConfigResetArgs {
    /// Reset specific key only (omit for full reset)
    pub key: Option<String>,

    /// Skip confirmation for full reset
    #[arg(long)]
    pub yes: bool,
}
```

### Examples

```bash
# Show full config
hypercolor config show

# Get a specific value
hypercolor config get daemon.fps

# Set a value
hypercolor config set daemon.fps 60

# Set with live hot-reload to running daemon
hypercolor config set audio.gain 1.5 --live

# Open in $EDITOR
hypercolor config edit

# Reset a single key to default
hypercolor config reset daemon.fps

# Reset entire config to defaults
hypercolor config reset --yes
```

### Output

**`hypercolor config show` (human):**

```
  # ~/.config/hypercolor/config.toml

  [daemon]
  fps = 60
  canvas_width = 320
  canvas_height = 200
  auto_discover = true

  [daemon.network]
  bind = "127.0.0.1"
  port = 9420

  [audio]
  source = "default"
  sample_rate = 48000
  fft_bins = 200
  gain = 1.0
  beat_sensitivity = 0.6

  [cli]
  default_format = "text"
  color = "auto"
  pager = "less -R"

  [tui]
  refresh_fps = 30
  show_led_preview = true
  show_audio_panel = true
  animations = true
  vim_mode = true
```

**`hypercolor config get daemon.fps` (human):**

```
60
```

**`hypercolor config set daemon.fps 30 --live` (human):**

```
  daemon.fps: 60 -> 30  (applied to running daemon)
```

**JSON (`hypercolor config show --json`):**

```json
{
  "daemon": {
    "fps": 60,
    "canvas_width": 320,
    "canvas_height": 200,
    "auto_discover": true,
    "network": {
      "bind": "127.0.0.1",
      "port": 9420
    }
  },
  "audio": {
    "source": "default",
    "sample_rate": 48000,
    "fft_bins": 200,
    "gain": 1.0,
    "beat_sensitivity": 0.6
  },
  "cli": {
    "default_format": "text",
    "color": "auto",
    "pager": "less -R"
  },
  "tui": {
    "refresh_fps": 30,
    "show_led_preview": true,
    "show_audio_panel": true,
    "animations": true,
    "vim_mode": true
  }
}
```

### Exit Codes

| Subcommand | `0` | Other |
|------------|-----|-------|
| `get` | Value found | `3` key not found |
| `set` | Value set | `4` invalid key or value |
| `reset` | Reset | `3` key not found |
| `edit` | Saved | `4` invalid TOML after edit |
| `show` | Displayed | *(always succeeds -- reads file directly)* |

---

## 16. `hypercolor plugin`

Manage Wasm effect plugins and extensions. Phase 2+ feature -- the command tree is present from v1 for forward compatibility.

### Clap Structure

```rust
#[derive(Args)]
pub struct PluginArgs {
    #[command(subcommand)]
    pub command: PluginCommand,
}

#[derive(Subcommand)]
pub enum PluginCommand {
    /// List installed plugins
    List,
    /// Install a plugin from a file path or URL
    Install(PluginInstallArgs),
    /// Remove an installed plugin
    Remove(PluginRemoveArgs),
    /// Update a plugin (or all plugins)
    Update(PluginUpdateArgs),
    /// Scaffold a new plugin project
    New(PluginNewArgs),
    /// Show detailed plugin information
    Info(PluginInfoArgs),
}

#[derive(Args)]
pub struct PluginInstallArgs {
    /// Plugin source: file path (.wasm), directory, or URL
    pub source: String,

    /// Trust the plugin without interactive confirmation
    #[arg(long)]
    pub trust: bool,
}

#[derive(Args)]
pub struct PluginRemoveArgs {
    /// Plugin name
    pub name: String,

    /// Skip confirmation
    #[arg(long)]
    pub yes: bool,
}

#[derive(Args)]
pub struct PluginUpdateArgs {
    /// Plugin name (omit to update all)
    pub name: Option<String>,
}

#[derive(Args)]
pub struct PluginNewArgs {
    /// Plugin project name
    pub name: String,

    /// Project template: effect, input, output
    #[arg(long, default_value = "effect")]
    pub template: PluginTemplate,

    /// Output directory (default: ./<name>)
    #[arg(long)]
    pub output: Option<PathBuf>,
}

#[derive(ValueEnum, Clone)]
pub enum PluginTemplate {
    Effect,
    Input,
    Output,
}

#[derive(Args)]
pub struct PluginInfoArgs {
    /// Plugin name
    pub name: String,
}
```

### Examples

```bash
# List installed plugins
hypercolor plugin list

# Install from file
hypercolor plugin install ./my-effect.wasm

# Install from URL
hypercolor plugin install https://effects.hypercolor.dev/galaxy-spiral-1.0.wasm

# Install, trusting without prompt
hypercolor plugin install ./effect.wasm --trust

# Remove a plugin
hypercolor plugin remove galaxy-spiral

# Update all plugins
hypercolor plugin update

# Update a specific plugin
hypercolor plugin update galaxy-spiral

# Scaffold a new effect plugin project
hypercolor plugin new my-awesome-effect --template effect

# Show plugin info
hypercolor plugin info galaxy-spiral
```

### Output

**`hypercolor plugin list` (human):**

```
  Plugin              Version    Type      Effects    Size
  ────────────────────────────────────────────────────────────
  galaxy-spiral       1.0.0      effect    1          48 KB
  firefly             2.1.0      effect    1          32 KB
  hue-bridge          0.3.0      output    0          120 KB

  3 plugins installed
```

**`hypercolor plugin info galaxy-spiral` (human):**

```
  Plugin: galaxy-spiral

  Version      1.0.0
  Type         effect
  Author       community/stargazer42
  Description  A mesmerizing galaxy spiral with configurable arm count
  Size         48 KB
  Installed    2026-02-28
  Source       https://effects.hypercolor.dev/galaxy-spiral-1.0.wasm

  Effects:
    Galaxy Spiral    6 controls, audio-reactive
```

**JSON:**

```json
{
  "name": "galaxy-spiral",
  "version": "1.0.0",
  "type": "effect",
  "author": "community/stargazer42",
  "description": "A mesmerizing galaxy spiral with configurable arm count",
  "size_bytes": 49152,
  "installed_at": "2026-02-28T14:30:00Z",
  "source": "https://effects.hypercolor.dev/galaxy-spiral-1.0.wasm",
  "effects": [
    {
      "name": "Galaxy Spiral",
      "controls": 6,
      "audio_reactive": true
    }
  ]
}
```

### Exit Codes

| Subcommand | `0` | Other |
|------------|-----|-------|
| `list` | Listed | `2` daemon unavailable |
| `install` | Installed | `4` invalid wasm, `6` already installed, `1` download failed |
| `remove` | Removed | `3` not found |
| `update` | Updated (or no updates) | `3` not found, `1` update failed |
| `new` | Project created | `6` directory exists |
| `info` | Displayed | `3` not found |

---

## 17. `hypercolor watch`

Stream live events and metrics from the daemon. Designed for monitoring dashboards, scripting pipelines, and debugging. Output is newline-delimited (one JSON object or text line per event).

### Clap Structure

```rust
#[derive(Args)]
pub struct WatchArgs {
    /// Output format for the stream
    #[arg(long, default_value = "text")]
    pub format: WatchFormat,

    /// Update rate for frame/metrics data (events are always push-based)
    #[arg(long, default_value = "10")]
    pub fps: u32,

    /// Filter by event type(s) (repeatable: effect, device, profile, scene, system, frame, audio)
    #[arg(long, short)]
    pub filter: Vec<String>,

    /// Include frame data (LED colors) in the stream
    #[arg(long)]
    pub frames: bool,

    /// Include audio spectrum data in the stream
    #[arg(long)]
    pub spectrum: bool,

    /// Include performance metrics in the stream
    #[arg(long)]
    pub metrics: bool,
}

#[derive(ValueEnum, Clone)]
pub enum WatchFormat {
    Text,
    Json,
    Csv,
}
```

### Examples

```bash
# Stream all events as text
hypercolor watch

# Stream as JSON (for piping to jq)
hypercolor watch --format json

# Only device events
hypercolor watch --filter device

# Only effect and profile changes
hypercolor watch --filter effect --filter profile

# Stream with performance metrics at 1fps
hypercolor watch --metrics --fps 1

# Stream LED frame data at 10fps as CSV (for analysis)
hypercolor watch --frames --fps 10 --format csv

# Pipe to jq for processing
hypercolor watch --format json | jq 'select(.type == "effect_changed") | .data'

# Include audio spectrum data
hypercolor watch --spectrum --fps 30
```

### Output

**Text format:**

```
20:32:01.482  DeviceConnected    WLED Living Room (120 LEDs)
20:32:01.500  EffectChanged      Rainbow Wave -> Aurora Drift
20:32:02.100  ProfileApplied     Evening
20:32:05.823  AudioBeat          confidence: 0.89, bpm: 128
```

**JSON format (JSONL -- one object per line):**

```jsonl
{"ts":"2026-03-01T20:32:01.482Z","type":"device_connected","data":{"device":"WLED Living Room","leds":120,"backend":"wled"}}
{"ts":"2026-03-01T20:32:01.500Z","type":"effect_changed","data":{"previous":"Rainbow Wave","current":"Aurora Drift","trigger":"profile"}}
{"ts":"2026-03-01T20:32:02.100Z","type":"profile_applied","data":{"profile":"Evening","effect":"Aurora Drift"}}
{"ts":"2026-03-01T20:32:05.823Z","type":"audio_beat","data":{"confidence":0.89,"bpm":128}}
```

**CSV format (with `--frames`):**

```csv
timestamp,device,led_index,r,g,b
2026-03-01T20:32:01.516Z,wled_living_room,0,255,0,128
2026-03-01T20:32:01.516Z,wled_living_room,1,253,2,130
...
```

### Exit Codes

| Code | Meaning |
|------|---------|
| `0` | Clean exit (user interrupt or pipe closed) |
| `2` | Daemon not running |
| `130` | Interrupted by Ctrl+C |

---

## 18. `hypercolor export`

Export configuration, profiles, layouts, and effects for backup or sharing.

### Clap Structure

```rust
#[derive(Args)]
pub struct ExportArgs {
    #[command(subcommand)]
    pub target: ExportTarget,
}

#[derive(Subcommand)]
pub enum ExportTarget {
    /// Export all profiles to a file
    Profiles(ExportFileArgs),
    /// Export all layouts to a file
    Layouts(ExportFileArgs),
    /// Export daemon configuration
    Config(ExportFileArgs),
    /// Full backup of everything
    All(ExportAllArgs),
}

#[derive(Args)]
pub struct ExportFileArgs {
    /// Output file path
    pub file: PathBuf,

    /// Overwrite existing file
    #[arg(long)]
    pub force: bool,
}

#[derive(Args)]
pub struct ExportAllArgs {
    /// Output directory for the backup
    pub dir: PathBuf,

    /// Overwrite existing files
    #[arg(long)]
    pub force: bool,
}
```

### Examples

```bash
# Export all profiles
hypercolor export profiles ~/hypercolor-profiles.toml

# Export layouts
hypercolor export layouts ~/hypercolor-layouts.toml

# Export config
hypercolor export config ~/hypercolor-config.toml

# Full backup to a directory
hypercolor export all ~/hypercolor-backup/

# Overwrite existing
hypercolor export all ~/hypercolor-backup/ --force
```

### Output

**Human:**

```
  Exported 7 profiles to ~/hypercolor-profiles.toml (4.2 KB)
```

```
  Full backup to ~/hypercolor-backup/
    profiles.toml     7 profiles    4.2 KB
    layouts.toml      2 layouts     1.8 KB
    scenes.toml       3 scenes      1.1 KB
    config.toml       daemon config 2.4 KB
    Total: 9.5 KB
```

**JSON:**

```json
{
  "exported": {
    "profiles": 7,
    "file": "/home/bliss/hypercolor-profiles.toml",
    "size_bytes": 4300
  }
}
```

### Exit Codes

| Code | Meaning |
|------|---------|
| `0` | Exported |
| `1` | Write error |
| `2` | Daemon unavailable (for live data) |
| `6` | File exists (without `--force`) |

---

## 19. `hypercolor import`

Import configuration, profiles, layouts, and effects from backup files.

### Clap Structure

```rust
#[derive(Args)]
pub struct ImportArgs {
    #[command(subcommand)]
    pub target: ImportTarget,
}

#[derive(Subcommand)]
pub enum ImportTarget {
    /// Import profiles from a TOML file
    Profiles(ImportFileArgs),
    /// Import layouts from a TOML file
    Layouts(ImportFileArgs),
    /// Import daemon configuration
    Config(ImportFileArgs),
    /// Import effect files from a directory
    Effects(ImportEffectsArgs),
    /// Restore a full backup
    All(ImportAllArgs),
}

#[derive(Args)]
pub struct ImportFileArgs {
    /// Input file path
    pub file: PathBuf,

    /// Overwrite existing resources with same names
    #[arg(long)]
    pub force: bool,

    /// Dry run -- show what would be imported without making changes
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Args)]
pub struct ImportEffectsArgs {
    /// Directory containing effect files (HTML/JS or .wasm)
    pub dir: PathBuf,
}

#[derive(Args)]
pub struct ImportAllArgs {
    /// Backup directory (from 'export all')
    pub dir: PathBuf,

    /// Overwrite existing resources
    #[arg(long)]
    pub force: bool,

    /// Dry run
    #[arg(long)]
    pub dry_run: bool,
}
```

### Examples

```bash
# Import profiles
hypercolor import profiles ~/hypercolor-profiles.toml

# Import with overwrite
hypercolor import profiles ~/hypercolor-profiles.toml --force

# Dry run -- see what would change
hypercolor import profiles ~/hypercolor-profiles.toml --dry-run

# Import layouts
hypercolor import layouts ~/hypercolor-layouts.toml

# Import effect files
hypercolor import effects ~/custom-effects/

# Restore full backup
hypercolor import all ~/hypercolor-backup/

# Full restore with overwrite
hypercolor import all ~/hypercolor-backup/ --force
```

### Output

**Human:**

```
  Imported 7 profiles from ~/hypercolor-profiles.toml
    New:       Gaming, Party Mode
    Updated:   Evening, Focus
    Skipped:   Movie Night, Sleep, All Off (already exist, use --force)
```

**Human (dry run):**

```
  Dry run -- no changes made

  Would import 7 profiles from ~/hypercolor-profiles.toml
    New:       Gaming, Party Mode
    Update:    Evening, Focus (use --force)
    Skip:      Movie Night, Sleep, All Off (already exist)
```

**JSON:**

```json
{
  "imported": {
    "new": ["Gaming", "Party Mode"],
    "updated": ["Evening", "Focus"],
    "skipped": ["Movie Night", "Sleep", "All Off"]
  },
  "source": "/home/bliss/hypercolor-profiles.toml",
  "dry_run": false
}
```

### Exit Codes

| Code | Meaning |
|------|---------|
| `0` | Imported (or dry run complete) |
| `1` | Read error or parse error |
| `4` | Invalid file format |
| `6` | Resources exist (without `--force`, and not all skipped) |

---

## 20. `hypercolor diagnose`

Run comprehensive system diagnostics. Checks daemon health, device connectivity, render pipeline, audio subsystem, and configuration validity. Designed for troubleshooting and bug reports.

### Clap Structure

```rust
#[derive(Args)]
pub struct DiagnoseArgs {
    /// Run specific check(s) only (repeatable: daemon, devices, audio, render, config, permissions)
    #[arg(long)]
    pub check: Vec<String>,

    /// Generate a full diagnostic report file for bug reports
    #[arg(long)]
    pub report: Option<PathBuf>,

    /// Include verbose system information (GPU, kernel, PipeWire version, etc.)
    #[arg(long)]
    pub system: bool,
}
```

### Examples

```bash
# Run all diagnostics
hypercolor diagnose

# Check only device connectivity
hypercolor diagnose --check devices

# Check audio subsystem
hypercolor diagnose --check audio

# Generate a report file for a bug report
hypercolor diagnose --report ~/hypercolor-diag.txt --system

# Multiple specific checks
hypercolor diagnose --check daemon --check render
```

### Output

**Human:**

```
  Hypercolor Diagnostics

  ── Daemon ────────────────────────────────────────
  ✓ Daemon running                    pid 4821
  ✓ Socket accessible                 /run/hypercolor/hypercolor.sock
  ✓ API responding                    http://127.0.0.1:9420
  ✓ Web UI accessible                 http://127.0.0.1:9420/

  ── Render Pipeline ───────────────────────────────
  ✓ wgpu initialized                  Vulkan 1.3 (AMD Radeon RX 7900 XTX)
  ✓ Render loop active                60.0 fps (target: 60)
  ✓ Frame budget                      16.4ms / 16.7ms (1.8% headroom)
  ! Canvas resolution                 320x200 (consider 640x400 for 5000+ LEDs)

  ── Devices ───────────────────────────────────────
  ✓ WLED Living Room                  connected, 0.8ms latency
  ✓ Prism 8 Controller                connected, 2.1ms latency
  ✓ Strimer ATX                       connected, 1.8ms latency
  ✓ Strimer GPU                       connected, 1.8ms latency

  ── Audio ─────────────────────────────────────────
  ✓ PipeWire available                0.3.85
  ✓ Audio capture active              48kHz stereo
  ✓ FFT processing                    200 bins, 0.3ms/frame
  ✓ Beat detection                    operational (sensitivity: 0.6)

  ── Configuration ─────────────────────────────────
  ✓ Config file valid                 ~/.config/hypercolor/config.toml
  ✓ Data directory                    ~/.local/share/hypercolor/
  ✓ Profile storage                   7 profiles, 12 KB

  ── Permissions ───────────────────────────────────
  ✓ USB HID access                    udev rules installed
  ✓ Socket permissions                user rw, group rw

  Summary: 17 passed, 1 warning, 0 failed
```

**JSON:**

```json
{
  "checks": [
    {
      "category": "daemon",
      "name": "daemon_running",
      "status": "pass",
      "detail": "pid 4821"
    },
    {
      "category": "render",
      "name": "frame_budget",
      "status": "pass",
      "detail": "16.4ms / 16.7ms (1.8% headroom)"
    },
    {
      "category": "render",
      "name": "canvas_resolution",
      "status": "warning",
      "detail": "320x200 (consider 640x400 for 5000+ LEDs)"
    }
  ],
  "summary": {
    "passed": 17,
    "warnings": 1,
    "failed": 0
  }
}
```

### Exit Codes

| Code | Meaning |
|------|---------|
| `0` | All checks passed (warnings are okay) |
| `1` | One or more checks failed |
| `2` | Daemon not running (daemon check itself fails) |

---

## 21. `hypercolor completion`

Generate shell completion scripts for bash, zsh, and fish. Uses clap's built-in completion generator with additional dynamic completions that query the daemon for resource names.

### Clap Structure

```rust
#[derive(Args)]
pub struct CompletionArgs {
    /// Shell to generate completions for
    pub shell: CompletionShell,
}

#[derive(ValueEnum, Clone)]
pub enum CompletionShell {
    Bash,
    Zsh,
    Fish,
}
```

### Examples

```bash
# Generate bash completions
hypercolor completion bash > /etc/bash_completion.d/hypercolor

# Generate zsh completions
hypercolor completion zsh > "${fpath[1]}/_hypercolor"

# Generate fish completions
hypercolor completion fish > ~/.config/fish/completions/hypercolor.fish
```

### Completion Coverage

| What | Type | Source |
|------|------|--------|
| Subcommands and flags | Static | clap derive |
| Effect names | Dynamic | Daemon query (`completions` RPC) |
| Device names | Dynamic | Daemon query |
| Profile names | Dynamic | Daemon query |
| Scene names | Dynamic | Daemon query |
| Config keys | Dynamic | Daemon query |
| Shell names (bash/zsh/fish) | Static | `ValueEnum` |
| Log levels | Static | `ValueEnum` |
| File paths (import/export) | Static | Shell built-in |

Dynamic completions gracefully degrade when the daemon is not running -- only static completions are available.

### Output

Completion script written to stdout. No human-readable wrapper. Pipe directly to the appropriate file.

### Exit Codes

| Code | Meaning |
|------|---------|
| `0` | Completion script generated |

---

## 22. `hypercolor setup`

First-time setup wizard. Walks the user through initial configuration: device discovery, audio setup, first profile creation, and optional web UI configuration.

### Clap Structure

```rust
#[derive(Args)]
pub struct SetupArgs {
    /// Non-interactive mode: auto-detect everything, use defaults
    #[arg(long)]
    pub auto: bool,

    /// Skip device discovery
    #[arg(long)]
    pub skip_devices: bool,

    /// Skip audio configuration
    #[arg(long)]
    pub skip_audio: bool,
}
```

### Examples

```bash
# Interactive setup wizard
hypercolor setup

# Fully automated setup (detect everything, use defaults)
hypercolor setup --auto

# Setup without device discovery (e.g., devices not connected yet)
hypercolor setup --skip-devices
```

### Output (Interactive)

```
  ╭─ Hypercolor Setup ─────────────────────────────────────────╮
  │                                                              │
  │   Welcome to Hypercolor! Let's get your lights configured.   │
  │                                                              │
  ╰──────────────────────────────────────────────────────────────╯

  Step 1/4: Device Discovery

  Scanning for RGB devices...
  ⠸ USB HID...    found 3
  ⠴ mDNS (WLED).. found 1

  Found 4 devices:
    [1] ✓  WLED Living Room        120 LEDs    DDP
    [2] ✓  Prism 8 Controller     1008 LEDs    HID
    [3] ✓  Strimer ATX             120 LEDs    HID
    [4] ✓  Strimer GPU             108 LEDs    HID

  Enable all? [Y/n] y

  Step 2/4: Audio Setup

  Detected audio system: PipeWire 0.3.85
  Default source: PipeWire Multimedia
  Test audio capture? [Y/n] y

  ♪ Listening... Level: ▇▇▇▇▇░░░ 58% — Audio working!

  Step 3/4: Initial Profile

  Save current state as your first profile?
  Profile name [Default]: Evening

  ✦ Profile saved: Evening

  Step 4/4: Daemon Configuration

  API port [9420]:
  Start daemon on boot (systemd)? [Y/n] y
  Enable web UI? [Y/n] y

  ╭─ Setup Complete ───────────────────────────────────────────╮
  │                                                              │
  │   Hypercolor is ready!                                       │
  │                                                              │
  │   Daemon:    hypercolor daemon start                         │
  │   TUI:       hypercolor tui                                  │
  │   Web UI:    http://127.0.0.1:9420/                          │
  │   Quick:     hypercolor set rainbow-wave                     │
  │                                                              │
  ╰──────────────────────────────────────────────────────────────╯
```

### Output (Auto Mode)

```
  Hypercolor Auto Setup

  ✓ Config created      ~/.config/hypercolor/config.toml
  ✓ Discovered          4 devices (1,356 LEDs)
  ✓ Audio detected      PipeWire 0.3.85
  ✓ Profile saved       Default
  ✓ Daemon registered   hypercolor.service (systemd user)

  Ready. Run 'hypercolor daemon start' to begin.
```

**JSON (`hypercolor setup --auto --json`):**

```json
{
  "config_path": "/home/bliss/.config/hypercolor/config.toml",
  "devices_found": 4,
  "total_leds": 1356,
  "audio_system": "PipeWire 0.3.85",
  "profile_created": "Default",
  "systemd_enabled": true
}
```

### Exit Codes

| Code | Meaning |
|------|---------|
| `0` | Setup complete |
| `1` | Setup failed or was cancelled |
| `4` | Invalid user input during wizard |

---

## 23. Output Design

### Principles

1. **Beautiful by default, machine-readable by flag.** Every command produces styled, SilkCircuit-themed output. Add `--json` for scripting.
2. **Quiet mode is silent success.** `--quiet` suppresses all decoration. Errors still print to stderr.
3. **Consistent color semantics.** The SilkCircuit Neon palette is used consistently across all commands.
4. **Stderr for errors, stdout for data.** All error/warning messages go to stderr. Data and results go to stdout.

### Color Semantics

| Element | Color | Hex |
|---------|-------|-----|
| Section titles, emphasis | Electric Purple (bold) | `#e135ff` |
| Paths, interactions, links | Neon Cyan | `#80ffea` |
| Numbers, data values, hashes | Coral | `#ff6ac1` |
| Warnings, timestamps | Electric Yellow | `#f1fa8c` |
| Success indicators, checkmarks | Success Green | `#50fa7b` |
| Errors, failure indicators | Error Red | `#ff6363` |
| Box borders, separators | Dim Electric Purple | `#e135ff` at 40% |
| Labels, secondary text | Dim white | `#6272a4` |
| Body text | Base white | `#f8f8f2` |

### Symbols

| Symbol | Meaning | Unicode |
|--------|---------|---------|
| `✦` | Effect applied / active | U+2726 |
| `●` | Status: connected/active | U+25CF |
| `○` | Status: off/inactive | U+25CB |
| `✓` | Check passed | U+2713 |
| `!` | Warning | U+0021 |
| `✗` | Error / check failed | U+2717 |
| `◈` | Web engine effect | U+25C8 |

### Error Format

Errors are actionable. Every error includes:
1. What went wrong (in Error Red).
2. Why (context).
3. What to do about it (suggestion).

```
  ✗ Cannot connect to daemon
    Socket /run/hypercolor/hypercolor.sock does not exist.
    Start the daemon with: hypercolor daemon start
```

```
  ✗ Device not found: "WLED Bedroom"
    Available devices:
      WLED Living Room
      Prism 8 Controller
    Did you mean "WLED Living Room"?
```

### Progress Indicators

Long operations use animated spinners from the `indicatif` crate:

```
  Discovering devices...
  ⠸ Scanning USB HID...         found 3
  ⠴ Scanning mDNS (WLED)...     found 2
  ⠦ Scanning WLED...  timeout
```

Spinners are suppressed in `--quiet` mode and when stdout is not a TTY.

---

## 24. NO_COLOR Compliance

Hypercolor fully complies with the [NO_COLOR](https://no-color.org) standard.

### Behavior

When `NO_COLOR` is set (to any value, including empty string):

- All ANSI color and style codes are stripped from output.
- Unicode symbols (checkmarks, status dots, box-drawing) are preserved -- they convey meaning beyond color.
- Layout and alignment are preserved.
- `--json` output is unaffected (it never contains ANSI codes).

### Additional Controls

| Mechanism | Scope | Behavior |
|-----------|-------|----------|
| `NO_COLOR` env var | Global | Disables color when set |
| `--no-color` flag | Per-invocation | Disables color |
| `HYPERCOLOR_COLOR=never` | Persistent | Disables color |
| `HYPERCOLOR_COLOR=always` | Persistent | Forces color even in pipes |
| TTY detection | Automatic | Disables color when stdout is not a TTY |

### Implementation

```rust
use supports_color::Stream;

pub fn should_use_color(cli: &Cli) -> bool {
    // Explicit disable flags take priority
    if cli.no_color || std::env::var("NO_COLOR").is_ok() {
        return false;
    }

    // Explicit enable
    if std::env::var("HYPERCOLOR_COLOR").as_deref() == Ok("always") {
        return true;
    }

    // Explicit disable via env
    if std::env::var("HYPERCOLOR_COLOR").as_deref() == Ok("never") {
        return false;
    }

    // Auto-detect: check if stdout supports color
    supports_color::on(Stream::Stdout).is_some()
}
```
