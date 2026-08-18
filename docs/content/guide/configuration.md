+++
title = "Configuration"
description = "The hypercolor.toml reference: every section and key, their defaults, and the CLI and REST config surface."
weight = 140
template = "page.html"
+++

Hypercolor reads its main configuration from a single TOML file. The daemon creates it with working defaults on first run, so you only need to edit it when you want to change behavior.

## Config file location

| Platform | Path |
|---|---|
| Linux | `~/.config/hypercolor/hypercolor.toml` |
| Windows | `%APPDATA%\hypercolor\hypercolor.toml` |
| macOS | `~/Library/Application Support/hypercolor/hypercolor.toml` |

On Linux the path follows the XDG Base Directory spec (`$XDG_CONFIG_HOME/hypercolor/`, defaulting to `~/.config/hypercolor/`). Point the daemon at a different file with the `--config` flag:

```bash
hypercolor-daemon --config /etc/hypercolor/hypercolor.toml
```

Print the path the CLI resolves (it honors the `HYPERCOLOR_CONFIG` environment variable):

```bash
hypercolor config path
```

The daemon creates the file on first run from compile-time defaults, then reads and writes that single file. There is no system-wide layering or include-merge step; every setting lives in one TOML document. CLI flags like `--listen` and `--bind` override the file for a single launch without rewriting it.

---

## Top-level sections

The root `HypercolorConfig` struct has these sections:

| Section | What it controls |
|---|---|
| `[daemon]` | Render loop, network binding, logging, canvas, lifecycle |
| `[web]` | Embedded web UI and WebSocket preview server |
| `[mcp]` | Model Context Protocol server (off by default) |
| `[effect_engine]` | Renderer selection, hot-reload, extra effect paths |
| `[rendering]` | Servo GPU import policy |
| `[media]` | Video/stream producer limits |
| `[audio]` | Audio capture device and FFT analysis |
| `[capture]` | Screen capture for ambient lighting |
| `[input]` | Host keyboard/mouse capture consent and routing |
| `[display]` | LCD face FPS cap |
| `[discovery]` | mDNS, scan interval, ROLI Blocks |
| `[network]` | Remote access modes and client scope |
| `[drivers.<id>]` | Per-driver enable/settings (keyed by driver ID) |
| `[dbus]` | D-Bus session bus integration (Linux) |
| `[tui]` | Terminal UI theme, preview FPS, keybindings |
| `[session]` | Idle, lock, suspend, and lid lighting behavior (Linux) |
| `[features]` | Opt-in experimental flags |

---

## `[daemon]`

Core render loop and server binding.

```toml
[daemon]
listen_address   = "127.0.0.1"       # Loopback only; "0.0.0.0" for all interfaces
port             = 9420               # REST + WebSocket + web UI port
unix_socket      = true               # Also expose a Unix domain socket
target_fps       = 30                 # Render loop target (10/20/30/45/60 tiers)
canvas_width     = 640                # Effect canvas width in pixels
canvas_height    = 480                # Effect canvas height in pixels
max_devices      = 32                 # Maximum simultaneous device connections
log_level        = "info"            # trace | debug | info | warn | error
log_file         = ""                 # Empty = stderr only; path enables file output
start_profile    = "last"             # "last" | "default" | <profile name>
shutdown_behavior = "hardware_default" # hardware_default | off | static
shutdown_color   = "#1a1a2e"          # Hex color used when shutdown_behavior = "static"
```

**`target_fps`** controls the maximum render cadence. The FPS controller auto-shifts between tiers (10, 20, 30, 45, 60) based on frame budget; this key sets the ceiling. Changes to this key take effect live without a daemon restart.

**`canvas_width` / `canvas_height`** are the render canvas dimensions. LED spatial positions are normalized to `[0.0, 1.0]`, so layouts remain valid across different canvas sizes. Canvas resizes take effect at the next frame boundary. Both keys support live reload.

**`start_profile`**: `"last"` restores the profile that was active at shutdown. `"default"` loads a profile named "default". Any other string is treated as a profile name.

**`shutdown_behavior`**: `hardware_default` leaves LEDs on their last hardware frame (most controllers hold it). `off` sends a black frame to every device. `static` sends the color in `shutdown_color`.

For one-off daemon launches, CLI flags override these without touching the file:

```bash
hypercolor-daemon --listen-all          # bind to every interface
hypercolor-daemon --listen 192.168.1.42 # specific interface, configured port
hypercolor-daemon --bind 0.0.0.0:9421   # explicit address and port
```

`--listen` sets the interface and keeps the configured port; `--bind` takes a full `address:port`. To change the port persistently, set `daemon.port` in the file.

---

## `[web]`

Controls the embedded web UI and WebSocket preview stream served on the daemon port.

```toml
[web]
enabled       = true    # Serve the web UI and REST API on the daemon port
open_browser  = false   # Auto-open browser when the daemon starts
cors_origins  = []      # Extra allowed CORS origins (only active with API key auth)
websocket_fps = 30      # LED preview frame rate pushed to WebSocket clients
```

The web UI is served at `http://localhost:9420`. Disabling `web.enabled` removes the UI routes but leaves the REST and WebSocket API intact. `cors_origins` only matters when `HYPERCOLOR_API_KEY` authentication is active.

---

## `[mcp]`

The MCP server is **disabled by default**. Enable it to let AI agents control Hypercolor via the Model Context Protocol. See [@/api/mcp.md](@/api/mcp.md) for the full setup guide.

```toml
[mcp]
enabled             = false   # Must be set to true to activate the MCP server
base_path           = "/mcp"  # URL prefix for the MCP endpoint
stateful_mode       = true    # Maintain persistent SSE session state
json_response       = false   # Use JSON responses instead of SSE framing
sse_keep_alive_secs = 15      # SSE heartbeat interval
```

Once enabled, the MCP server exposes 17 tools, 5 resources, and 3 prompts at `http://localhost:9420/mcp`.

---

## `[effect_engine]`

Controls which renderer handles effects and how new effects are loaded.

```toml
[effect_engine]
preferred_renderer           = "auto"   # "auto" | "servo" | "wgpu"
servo_enabled                = true     # Enable Servo path for HTML/Canvas effects
wgpu_backend                 = "auto"   # "auto" | "vulkan" | "opengl"
compositor_acceleration_mode = "auto"   # "cpu" | "auto" | "gpu"
effect_error_fallback        = "none"   # "none" | "clear_groups"
extra_effect_dirs            = []       # Additional directories scanned for effects
watch_effects                = true     # Hot-reload effects on file change
watch_config                 = true     # Hot-reload hypercolor.toml on file change
```

**`preferred_renderer`**: `"auto"` selects the best available path. HTML and TypeScript effects require Servo (`servo_enabled = true`). Native Rust effects bypass this setting.

**`compositor_acceleration_mode`**: governs the scene-composition path that blends producer surfaces into the final canvas. `"auto"` tries GPU acceleration and falls back to CPU transparently. The key formerly appeared as `render_acceleration_mode` in older configs, and the daemon normalizes that name automatically.

**`extra_effect_dirs`**: list of absolute or config-relative paths. Each directory is scanned for `.html` effect bundles on startup and watched for changes when `watch_effects = true`.

{% callout(type="tip") %}
Add your effect development directory here to get live hot-reload without a daemon restart:

```toml
[effect_engine]
extra_effect_dirs = ["/home/you/dev/my-effects/dist"]
```
{% end %}

---

## `[rendering]`

Policy for Servo GPU framebuffer import (zero-copy texture sharing into the compositor). GPU import ships on all three platforms.

```toml
[rendering]
[rendering.servo_gpu_import]
mode = "auto"   # "off" | "auto" | "on"
```

`"auto"` attempts zero-copy GPU import when startup capability checks pass and falls back to CPU readback silently. `"on"` requires import and surfaces frame errors instead of falling back. Use `"off"` if you see rendering corruption with GPU compositing enabled.

---

## `[media]`

Resource caps for video and livestream producers in effects.

```toml
[media]
max_video_producers            = 2    # Maximum concurrent video file producers
max_livestream_producers       = 1    # Maximum concurrent livestream producers
stream_private_network_allowlist = [] # Private network URLs allowed in stream effects
```

---

## `[audio]`

Audio capture for reactive effects. See [@/guide/audio-setup.md](@/guide/audio-setup.md) for device discovery and troubleshooting.

```toml
[audio]
enabled          = true     # Enable audio capture
device           = "default" # PulseAudio/PipeWire device, "default", or "microphone"
fft_size         = 1024     # FFT window size: 256 | 512 | 1024 | 2048 | 4096
smoothing        = 0.8      # FFT smoothing (0.0 = raw signal, 1.0 = fully frozen)
noise_gate       = 0.02     # Signal below this level is treated as silence
beat_sensitivity = 0.6      # Beat detection threshold (0.0 = never, 1.0 = always)
```

**`device`** special values: `"default"` captures from the system monitor (what you hear); `"microphone"` uses the default input device; any other string is matched against PulseAudio/PipeWire device names. List available devices:

```bash
curl http://localhost:9420/api/v1/audio/devices | jq
```

**`fft_size`** must be a power of two. Smaller values give faster response, larger values give better low-frequency resolution. 1024 is a good default for most music.

Audio config changes take effect immediately through `config set` or the REST API, and the daemon reconfigures the input pipeline without restarting. Pass `--no-live` (or `?live=false`) to persist the value without disturbing the running pipeline.

---

## `[capture]`

Screen capture for ambient lighting effects. On Windows it is on by default: DXGI Desktop Duplication asks for no permission, shows no picker, and draws no capture indicator, so an ambient effect works immediately. On Linux it is opt-in: Wayland capture goes through the XDG desktop portal and PipeWire, which opens a picker, and answering it on your behalf at daemon start would be an ambush. X11 sessions have no capture path. macOS has no screen capture at all, and setting `capture.enabled = true` there is rejected by config validation.

```toml
[capture]
enabled                = false    # Grant screen capture (default: true on Windows)
source                 = "auto"   # "auto", or a monitor selector on Windows
capture_fps            = 30       # Capture rate, independent of render FPS
grid_cols              = 8        # Ambilight sector grid columns
grid_rows              = 6        # Ambilight sector grid rows
smoothing              = 0.3      # Temporal smoothing (0.0 = frozen, 1.0 = raw)
scene_cut_threshold    = 100.0    # Frame-difference that bypasses smoothing on scene cuts
letterbox              = false    # Auto-detect and crop black bars
letterbox_threshold    = 0.02     # Luminance threshold for bar detection
saturation             = 1.0      # Saturation boost applied to zone colors
brightness             = 1.0      # Brightness multiplier applied to zone colors
gamma                  = 1.0      # Gamma shaping (1.0 = neutral, >1 darkens midtones)
# publication_memory_bytes        # Optional byte budget; unset snapshots host memory at startup
```

**`enabled`** grants permission and nothing more. The capture backend opens on demand and stays closed until a screen-reactive effect actually asks for pixels.

**`source`** must be `"auto"` on Linux: the XDG desktop portal owns the selection, and the chosen source is persisted in `restore_token` (written automatically) so it survives daemon restarts without re-prompting. On Windows the value addresses a display directly, either `"auto"` for the primary output or a monitor selector such as `monitor:<stable-id>`. A bare number or `display:<n>` is accepted as a legacy output index and rewritten to its stable form once resolved.

**`letterbox`** is off by default. Ambient lighting almost always mirrors a desktop rather than a letterboxed film, and dark desktop content trips the bar detector into cropping real picture away. Turn it on when you are mirroring video that genuinely has bars.

**`publication_memory_bytes`** is an optional process-memory byte budget shared by capture analysis and screen publications. Leave it unset and the daemon snapshots available host memory at startup. Capture dimensions are not capped by a fixed axis or pixel count: the analyzer reserves its peak first, publication plans consume the remainder, and checked memory and compute admission decide whether a requested configuration fits. A configuration that does not fit is rejected with a typed capacity error rather than silently clamped. `/api/v1/status` reports the installed fences under `screen_capture_capacity`, whose `admission_enforced` flag tells you whether the budget is active on this host.

Capture config changes apply live: enabling/disabling adds or removes the source from the running pipeline; grid, smoothing, and color settings reconfigure the capture worker in place.

---

## `[input]`

Host keyboard and mouse capture for interactive effects. Capture is consent-gated and off by default; see [Input capture](@/guide/input-capture.md) for the full consent and platform model.

```toml
[input]
enabled       = false      # Master consent switch for host input capture
keyboard      = true       # Capture key positions (never typed text)
mouse         = true       # Capture pointer motion, buttons, and wheel
daemon_route  = "host"     # Input source for device output: host | browser | merge
preview_route = "browser"  # Input source for interactive previews: host | browser | merge
```

`keyboard` and `mouse` are independent grants; declining one means that input path is never opened at all. The route keys choose where interaction comes from. `host` uses captured hardware input, `browser` uses input addressed from a web UI preview, and `merge` combines both. `daemon_route` feeds authoritative device output; `preview_route` feeds each interactive browser preview.

---

## `[display]`

Controls LCD face (device display panel) rendering.

```toml
[display]
face_fps_cap = 30   # Upper FPS bound for HTML face rendering (15-60)
```

The device transport limit wins below this cap. Clamped to the range `[15, 60]`.

---

## `[discovery]`

Network device discovery settings.

```toml
[discovery]
background_enabled  = true    # Run startup, hotplug, and periodic discovery
mdns_enabled        = true    # Auto-detect mDNS-advertised devices (WLED, Hue, Nanoleaf)
scan_interval_secs  = 300     # Re-scan interval in seconds
blocks_scan         = true    # Enable ROLI Blocks discovery via blocksd bridge
blocks_socket_path  = ""      # Custom blocksd socket path (empty = auto-detect)
```

Discovery triggers automatically on startup, USB hotplug, and then at each `scan_interval_secs` interval. Set `background_enabled = false` to disable all three automatic triggers while keeping manual discovery available. Lower the interval if you frequently plug in and unplug network devices; raise it on battery-constrained setups.

---

## `[network]`

Remote access configuration for the daemon API.

```toml
[network]
access_mode                       = "local_only"    # local_only | lan_trusted | lan_protected | custom
client_scope                      = "local_subnets" # local_subnets | private_ranges | custom
mdns_publish                      = true            # Advertise daemon over mDNS
remote_access                     = false           # Legacy: open API to non-loopback clients
allow_unauthenticated_remote_access = false         # Permit API calls without auth from network
allowed_clients                   = []              # IP/CIDR allowlist (e.g. ["192.168.1.0/24"])
instance_name                     = ""              # mDNS instance name (defaults to hostname)
```

**Access modes:**

| Mode | Binding | Auth required |
|---|---|---|
| `local_only` | Loopback only | No |
| `lan_trusted` | All interfaces | No (anyone on the LAN can control it) |
| `lan_protected` | All interfaces | Yes (API key required) |
| `custom` | All interfaces | Controlled by `allow_unauthenticated_remote_access` and `allowed_clients` |

{% callout(type="warning") %}
`lan_trusted` exposes full control to anyone on your network with no authentication. Use `lan_protected` and set `HYPERCOLOR_API_KEY` if you need LAN access with some protection.
{% end %}

---

## `[drivers.<id>]`

Per-driver settings are namespaced by driver ID under a `[drivers]` table. Each entry has an `enabled` flag plus driver-owned settings flattened in:

```toml
[drivers.govee]
enabled              = true
known_ips            = ["192.168.1.50", "192.168.1.51"]  # Always probed during discovery
power_off_on_disconnect = false
lan_state_fps        = 10   # Maximum LAN whole-device state command rate
razer_fps            = 25   # Maximum Razer/Desktop streaming frame rate
```

Disable a driver entirely:

```toml
[drivers.wled]
enabled = false
```

Driver IDs correspond to the names registered in the driver registry. Available IDs include `govee`, `hue`, `nanoleaf`, `wled`, plus compile-time HAL driver IDs for USB/HID families.

---

## `[dbus]`

D-Bus integration for Linux desktop session events (screen lock, media players, power events).

```toml
[dbus]
enabled  = true
bus_name = "tech.hyperbliss.hypercolor1"
```

---

## `[tui]`

Terminal UI preferences.

```toml
[tui]
theme        = "silkcircuit"  # "silkcircuit" | "default" | "minimal"
preview_fps  = 15             # LED preview refresh rate in the TUI canvas
keybindings  = "default"      # "default" | "vim" | path to a custom keymap file
```

---

## `[session]`

Desktop session and power awareness: idle dimming, screen lock, suspend, and laptop lid behavior. Session events fire on Linux only today; on Windows and macOS these settings are inert.

```toml
[session]
enabled = true    # Master switch for session awareness
```

**Idle dimming.** After a period of no user activity, output dims, then turns off:

```toml
idle_enabled          = true     # React to idle/active transitions
idle_backend          = "auto"   # auto | wayland | x11 | dbus | disabled
idle_dim_timeout_secs = 120      # Idle seconds before dimming
idle_off_timeout_secs = 600      # Idle seconds before turning output off
```

**Lock, suspend, and lid behaviors.** Each event maps to a behavior: `off` (fade out and stop output frames), `dim` (drop to a brightness multiplier), `scene` (activate a named scene), or `ignore`.

```toml
on_screen_lock         = "ignore"  # off | dim | scene | ignore
screen_lock_brightness = 0.0       # Dim target when on_screen_lock = "dim"
screen_lock_scene      = ""        # Scene name when on_screen_lock = "scene"
screen_lock_fade_ms    = 2000
screen_unlock_fade_ms  = 500

on_suspend      = "off"            # off | dim | scene | ignore
suspend_fade_ms = 300
resume_fade_ms  = 150

on_lid_close         = "off"       # off | dim | scene | ignore
lid_close_brightness = 0.0
lid_close_scene      = ""
lid_close_fade_ms    = 500
lid_open_fade_ms     = 300
```

**Off-output behavior.** When an `off`-style action fires, these keys decide what devices do while asleep:

```toml
off_output_behavior = "static"    # static | release
off_output_color    = "#000000"   # Frame held when off_output_behavior = "static"
```

`static` holds a solid color frame (black by default) on the device; `release` disconnects and lets the device return to its native firmware behavior.

---

## `[features]`

Opt-in experimental features. All default to `false`.

```toml
[features]
wasm_plugins     = false  # Experimental WASM effect plugin system
hue_entertainment = false  # Philips Hue Entertainment API (low-latency streaming)
midi_input       = false  # MIDI controller input for effect control
```

---

## Authentication

When the `HYPERCOLOR_API_KEY` environment variable is set on the daemon, all API requests must include it:

```bash
curl -H "Authorization: Bearer <your-key>" http://localhost:9420/api/v1/status
```

The CLI reads the same variable, or you can pass it via `--api-key`:

```bash
hypercolor --api-key <your-key> effects list
```

A read-only key can be set with `HYPERCOLOR_READ_API_KEY` for clients that should be able to observe state but not change it.

---

## CLI config commands

The `hypercolor config` subcommand reads and writes the TOML file and optionally pushes changes to the running daemon:

```bash
# Show the full effective config as JSON
hypercolor config show

# Read a dotted key
hypercolor config get daemon.target_fps

# Write a key (persisted, and applied live when the key allows it)
hypercolor config set daemon.target_fps 60

# Persist a key without touching the running daemon
hypercolor config set audio.device "alsa_output.usb-Focusrite-monitor" --no-live

# Reset one key to its default
hypercolor config reset audio.smoothing

# Reset the entire config to defaults (requires --yes)
hypercolor config reset --yes
```

**Connection profiles** let the CLI target different daemon instances:

```bash
# Add a profile for a remote daemon
hypercolor config profile add remote --host 192.168.1.42 --port 9420 --api-key <key>

# Set it as default
hypercolor config profile default remote

# List profiles
hypercolor config profile list
```

---

## REST config API

```bash
# Get the full config
curl http://localhost:9420/api/v1/config | jq

# Read a single key
curl http://localhost:9420/api/v1/config/keys/audio.device | jq

# Set a key (the body is the value; add ?live=false to persist only)
curl -X PUT http://localhost:9420/api/v1/config/keys/daemon.target_fps \
  -H "Content-Type: application/json" \
  -d '60'

# Reset a key to its default
curl -X DELETE http://localhost:9420/api/v1/config/keys/audio.smoothing

# Full config reset
curl -X POST http://localhost:9420/api/v1/config/reset

# Ask how every key applies, renders, and validates
curl http://localhost:9420/api/v1/config/schema | jq
```

Keys are addressed with dotted paths matching the TOML structure (`daemon.target_fps`, `audio.device`, `drivers.govee.known_ips`, etc.), each a single path segment. A write returns the key's canonicalized effective value, a `"live"` boolean saying whether the running daemon re-applied it, a `"requires_restart"` boolean from the key registry, and `"pending_restart"`, the sections whose persisted values now differ from the ones the daemon booted with.

Reads mask what the key registry classifies as secret. Every `drivers` entry renders as `{"redacted": true}` on `/api/v1/config` and on a key read; driver settings are read and edited through `/api/v1/drivers/{id}/config` instead.

{% callout(type="tip") %}
Which keys reload live is the key registry's answer, not a list to memorize: `GET /api/v1/config/schema` reports `live` (with the subsystem the daemon re-applies), `live_on_read`, `next_scan`, `restart`, or `inert` for every key. Today the live sections are audio, screen capture, host input, and the render loop (`daemon.target_fps`, `daemon.canvas_width`, `daemon.canvas_height`). Every live section honors the same `?live=` flag, which defaults to applying; the CLI's `--live` maps onto it.
{% end %}
