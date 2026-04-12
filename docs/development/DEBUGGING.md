# Hypercolor Debugging Guide

Comprehensive reference for every debugging, diagnostics, and introspection tool
available in Hypercolor. Covers CLI, REST API, WebSocket, MCP AI tools, tracing,
packet inspection, mock backends, and development recipes.

---

## Table of Contents

- [CLI Diagnostics (`hypercolor diagnose`)](#cli-diagnostics)
- [Debug Discovery Tool (`hypercolor-debug`)](#debug-discovery-tool)
- [REST API Diagnostics](#rest-api-diagnostics)
- [Device Debug Endpoints](#device-debug-endpoints)
- [WebSocket Real-Time Monitoring](#websocket-real-time-monitoring)
- [MCP Tools (AI Agent Interface)](#mcp-tools)
- [Tracing & Logging](#tracing--logging)
- [Packet-Level Inspection](#packet-level-inspection)
- [Protocol Diagnostics Traits](#protocol-diagnostics-traits)
- [Mock Backend (Testing Without Hardware)](#mock-backend)
- [justfile Recipes](#justfile-recipes)
- [udev Setup](#udev-setup)
- [Common Debug Scenarios](#common-debug-scenarios)

---

## CLI Diagnostics

**Binary:** `hypercolor` (hypercolor-cli)
**Source:** `crates/hypercolor-cli/src/commands/diagnose.rs`

Run system-wide health checks from the terminal. Talks to the daemon's
`/api/v1/diagnose` endpoint and formats the results.

```bash
hypercolor diagnose [OPTIONS]
```

### Flags

| Flag | Type | Description |
|------|------|-------------|
| `--check <CHECK>` | repeatable | Run specific check(s) only. Values: `daemon`, `devices`, `audio`, `render`, `config`, `permissions` |
| `--report <PATH>` | path | Write full diagnostic JSON to a file (for bug reports) |
| `--system` | bool | Include verbose system info (GPU, kernel, audio version) |
| `--format <FMT>` | enum | Output format: `table` (default), `plain`, `json` |

### Examples

```bash
# Full system diagnostics with styled table output
hypercolor diagnose --system

# Check only the render loop and device registry
hypercolor diagnose --check render --check devices

# Generate a bug report file
hypercolor diagnose --system --report /tmp/hypercolor-diag.json

# Machine-readable JSON
hypercolor diagnose --format json
```

### Output

Table mode uses SilkCircuit-colored status icons:

```
  Hypercolor Diagnostics

  ── system ──────────────────────────────────────────
  ✓ daemon running                 0.1.0
  ── render ──────────────────────────────────────────
  ✓ render loop                    state=running, tier=high
  ── devices ─────────────────────────────────────────
  ✓ registry                      5 tracked
  ── config ──────────────────────────────────────────
  ✓ config manager                 available

  Summary: 4 passed, 0 warnings, 0 failed
```

| Icon | Color | Meaning |
|------|-------|---------|
| ✓ | `#50fa7b` (Success Green) | Pass |
| ! | `#f1fa8c` (Electric Yellow) | Warning |
| ✗ | `#ff6363` (Error Red) | Fail |

---

## Debug Discovery Tool

**Binary:** `hypercolor-debug`
**Source:** `crates/hypercolor-daemon/src/bin/hypercolor-debug.rs`

Standalone binary for debugging device discovery and USB hotplug events. Runs
outside the daemon — useful for isolating detection issues.

```bash
cargo run -p hypercolor-daemon --bin hypercolor-debug -- <COMMAND> [OPTIONS]
```

### Subcommands

#### `detect` — Run Discovery Sweeps

Continuously scans for devices and reacts to USB hotplug events.

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--backends <LIST>` | comma-separated | `usb,smbus` | Backends to scan: `usb`, `smbus`, `wled` |
| `--interval-secs <N>` | u64 | `5` | Periodic scan interval |
| `--duration-secs <N>` | u64 | ∞ | Auto-stop after N seconds |
| `--no-hotplug` | bool | `false` | Disable USB hotplug-triggered rescans |
| `--timeout-ms <N>` | u64 | `10000` | Network scanner timeout (WLED) |
| `--log-level <LVL>` | string | `info` | Log level: `trace`, `debug`, `info`, `warn`, `error` |

### Examples

```bash
# Scan USB + SMBus every 5 seconds with debug logging
cargo run -p hypercolor-daemon --bin hypercolor-debug -- \
  --log-level debug detect

# Scan only USB, stop after 30 seconds
cargo run -p hypercolor-daemon --bin hypercolor-debug -- \
  detect --backends usb --duration-secs 30

# Include WLED network discovery with 5s timeout
cargo run -p hypercolor-daemon --bin hypercolor-debug -- \
  detect --backends usb,smbus,wled --timeout-ms 5000

# Disable hotplug, periodic scans only
cargo run -p hypercolor-daemon --bin hypercolor-debug -- \
  detect --no-hotplug --interval-secs 2
```

### Output

```
[14:32:05.123] starting debug detection loop backends=[Usb, SmBus] interval_secs=5 timeout_ms=10000 hotplug=true
[14:32:05.456] scan trigger=initial new=3 reappeared=0 vanished=0 total=3 duration_ms=332
  scanner=UsbScanner status=ok discovered=2 duration_ms=120
  scanner=SmBusScanner status=ok discovered=1 duration_ms=331
  + Razer BlackWidow V4 Pro [a1b2c3d4] backend_hint=usb
  + Razer DeathAdder V3 Pro [e5f6a7b8] backend_hint=usb
  + ASUS Aura Motherboard [c9d0e1f2] backend_hint=smbus
[14:32:10.789] hotplug arrived 1532:0272 Razer Huntsman V3 Pro
[14:32:11.012] scan trigger=usb-hotplug new=1 reappeared=0 vanished=0 total=4 duration_ms=223
  + Razer Huntsman V3 Pro [3a4b5c6d] backend_hint=usb
[14:32:15.456] hotplug removed 1532:0272
```

### What It Reports

- **Per-scanner status:** ok/error, discovered count, scan duration
- **New devices:** `+` prefix with name, ID, backend hint
- **Reappeared devices:** `~` prefix (previously vanished, now back)
- **Vanished devices:** `-` prefix (previously seen, now gone)
- **Hotplug events:** Real-time USB arrival/removal with VID:PID

---

## REST API Diagnostics

**Base URL:** `http://localhost:9420/api/v1`

### `POST /api/v1/diagnose`

**Source:** `crates/hypercolor-daemon/src/api/diagnose.rs`

Run lightweight daemon diagnostics. All responses are wrapped in the standard
API envelope.

#### Request

```json
{
  "checks": ["daemon", "render", "devices", "config"],
  "system": true
}
```

Both fields are optional. Omitting `checks` runs all four default checks.
Setting `system: true` adds uptime information.

#### Response

```json
{
  "checks": [
    {
      "category": "system",
      "name": "daemon_running",
      "status": "pass",
      "detail": "0.1.0"
    },
    {
      "category": "render",
      "name": "render_loop",
      "status": "pass",
      "detail": "state=running, tier=high"
    },
    {
      "category": "devices",
      "name": "registry",
      "status": "pass",
      "detail": "5 tracked"
    },
    {
      "category": "config",
      "name": "config_manager",
      "status": "pass",
      "detail": "available"
    },
    {
      "category": "system",
      "name": "uptime_seconds",
      "status": "pass",
      "detail": "3600"
    }
  ],
  "summary": {
    "passed": 5,
    "warnings": 0,
    "failed": 0
  }
}
```

#### Available Checks

| Check | Category | What It Tests |
|-------|----------|---------------|
| `daemon` | system | Daemon version and running status |
| `render` | render | Render loop state and performance tier |
| `devices` | devices | Device registry count |
| `config` | config | Config manager availability (vs default/test state) |

#### Status Values

- `pass` — healthy
- `warning` — degraded (e.g., render loop stopped, config using defaults)
- `fail` — broken

#### Quick Test

```bash
# All checks
curl -s -X POST http://localhost:9420/api/v1/diagnose | jq .

# Specific checks with system info
curl -s -X POST http://localhost:9420/api/v1/diagnose \
  -H "Content-Type: application/json" \
  -d '{"checks": ["render", "devices"], "system": true}' | jq .
```

---

## Device Debug Endpoints

**Source:** `crates/hypercolor-daemon/src/api/devices.rs`

### `GET /api/v1/devices/debug/queues`

Inspect the backend output frame queue state. Shows pending frames, drop
counts, and last write timing for each device backend.

```bash
curl -s http://localhost:9420/api/v1/devices/debug/queues | jq .
```

Returns the result of `BackendManager::debug_snapshot()` — the exact shape
depends on the backend manager implementation, but exposes per-device queue
depth and throughput information.

### `GET /api/v1/devices/debug/routing`

Inspect how layout zones map to physical device backends. Shows the routing
table the render pipeline uses to dispatch LED frames.

```bash
curl -s http://localhost:9420/api/v1/devices/debug/routing | jq .
```

Returns the result of `BackendManager::routing_snapshot()` — shows which layout
device IDs route to which physical backends, with segment ranges and zone
mappings.

### `POST /api/v1/devices/{id}/identify`

Flash a device with a color for visual identification. Useful when you have
multiple devices of the same type and need to figure out which is which.

```bash
curl -s -X POST http://localhost:9420/api/v1/devices/abc123/identify \
  -H "Content-Type: application/json" \
  -d '{"duration_ms": 3000, "color": "#ffffff"}'
```

### Other Useful Device Endpoints

| Endpoint | Method | Purpose |
|----------|--------|---------|
| `/api/v1/devices` | GET | List all connected devices with state, zones, LED count |
| `/api/v1/devices/{id}` | GET | Single device detail |
| `/api/v1/devices/discover` | POST | Trigger a discovery scan |

---

## WebSocket Real-Time Monitoring

**Route:** `ws://localhost:9420/api/v1/ws`
**Protocol Version:** `1.0`
**Source:** `crates/hypercolor-daemon/src/api/ws.rs`

Bidirectional WebSocket for real-time event streaming, binary frame data,
performance metrics, and REST-equivalent command execution.

### Channels

| Channel | Default | Description | Config Options |
|---------|---------|-------------|----------------|
| `events` | subscribed | System events and state changes | — |
| `frames` | unsubscribed | Per-zone LED color frames | `fps` (1-60), `format` (binary/json), `zones` |
| `spectrum` | unsubscribed | Audio spectrum data | `fps` (1-60), `bins` (8/16/32/64/128) |
| `canvas` | unsubscribed | Rendered effect canvas pixels | `fps` (1-60), `format` (rgb/rgba) |
| `metrics` | unsubscribed | Performance metrics snapshots | `interval_ms` (100-10000) |

### Client → Server Messages

**Subscribe:**

```json
{
  "type": "subscribe",
  "channels": ["frames", "metrics"],
  "config": {
    "frames": { "fps": 30, "format": "binary", "zones": ["all"] },
    "metrics": { "interval_ms": 500 }
  }
}
```

**Unsubscribe:**

```json
{
  "type": "unsubscribe",
  "channels": ["frames"]
}
```

**Command (REST-over-WS):**

```json
{
  "type": "command",
  "id": "req-001",
  "method": "GET",
  "path": "/api/v1/devices",
  "body": null
}
```

### Server → Client Messages

**Hello (sent on connect):**

```json
{
  "type": "hello",
  "version": "1.0",
  "state": {
    "running": true,
    "paused": false,
    "brightness": 80,
    "fps": { "target": 60, "actual": 59.8 },
    "effect": { "id": "borealis-01", "name": "Borealis" },
    "profile": { "id": "default", "name": "Default" },
    "layout": { "id": "main", "name": "Main Setup" },
    "device_count": 5,
    "total_leds": 1200
  },
  "capabilities": ["frames", "spectrum", "events", "canvas", "metrics", "commands"],
  "subscriptions": ["events"]
}
```

**Event:**

```json
{
  "type": "event",
  "event": "device_connected",
  "timestamp": "2026-03-08T10:30:45.123Z",
  "data": { "device_id": "abc123", "name": "Razer BlackWidow V4 Pro" }
}
```

**Metrics:**

```json
{
  "type": "metrics",
  "timestamp": "2026-03-08T10:30:45.123Z",
  "data": {
    "fps": { "target": 60, "actual": 59.8, "dropped": 0 },
    "frame_time": { "avg_ms": 16.8, "p95_ms": 18.2, "p99_ms": 19.1, "max_ms": 22.3 },
    "stages": {
      "input_sampling_ms": 2.1,
      "effect_rendering_ms": 5.3,
      "spatial_sampling_ms": 1.2,
      "device_output_ms": 8.2,
      "event_bus_ms": 0.1
    },
    "memory": { "daemon_rss_mb": 45.2, "servo_rss_mb": 128.5, "canvas_buffer_kb": 512 },
    "devices": { "connected": 5, "total_leds": 1200, "output_errors": 0 },
    "websocket": { "client_count": 2, "bytes_sent_per_sec": 1024.5 }
  }
}
```

**Backpressure:**

```json
{
  "type": "backpressure",
  "dropped_frames": 12,
  "channel": "frames",
  "recommendation": "Reduce fps or enable selective frame filtering",
  "suggested_fps": 15
}
```

**Error:**

```json
{
  "type": "error",
  "code": "invalid_config",
  "message": "Invalid configuration for config.frames.fps: expected 1..=60",
  "details": { "field": "config.frames.fps", "reason": "expected 1..=60" }
}
```

**Command Response:**

```json
{
  "type": "response",
  "id": "req-001",
  "status": 200,
  "data": [...],
  "error": null
}
```

### Connection Parameters

| Parameter | Value |
|-----------|-------|
| Buffer size | 64 events per client |
| Ping interval | 30 seconds |
| Pong timeout | 10 seconds |
| Overflow behavior | Binary frames dropped (backpressure warning sent) |

### Quick Test with websocat

```bash
# Connect and see the hello message
websocat ws://localhost:9420/api/v1/ws

# Subscribe to metrics every 500ms
echo '{"type":"subscribe","channels":["metrics"],"config":{"metrics":{"interval_ms":500}}}' | \
  websocat ws://localhost:9420/api/v1/ws
```

---

## MCP Tools

**Source:** `crates/hypercolor-daemon/src/mcp/tools.rs`

Hypercolor exposes 14 MCP tools for AI assistant integration. These give agents
programmatic control over the lighting system.

### Tool Inventory

| Tool | Read-Only | Description |
|------|-----------|-------------|
| `set_effect` | no | Apply a lighting effect (supports fuzzy/natural language matching) |
| `list_effects` | yes | Browse the effect library with category/audio filters |
| `stop_effect` | no | Stop the current effect |
| `set_color` | no | Set a static color on devices |
| `get_devices` | yes | List connected devices |
| `set_brightness` | no | Set brightness (0-100) |
| `get_status` | yes | Current daemon state snapshot |
| `activate_scene` | no | Activate a saved scene |
| `list_scenes` | yes | List available scenes |
| `create_scene` | no | Create a new scene from current state |
| `get_audio_state` | yes | Audio input and spectrum data |
| `set_profile` | no | Switch device profile |
| `get_layout` | yes | Current layout mapping |
| **`diagnose`** | **yes** | **System/device diagnostics** |

### `diagnose` Tool (Detail)

The most relevant tool for debugging. Runs targeted diagnostics on the system
or a specific device.

**Input:**

```json
{
  "device_id": "optional-device-id",
  "checks": ["connectivity", "latency", "frame_delivery", "color_accuracy", "protocol", "all"]
}
```

**Output:**

```json
{
  "overall_status": "healthy",
  "findings": [
    { "severity": "info", "message": "All 5 devices connected and responding" },
    { "severity": "info", "message": "Frame delivery rate: 59.8/60 fps" }
  ],
  "metrics": {
    "fps": 59.8,
    "frame_drop_rate": 0.0,
    "avg_latency_ms": 16.8,
    "device_error_count": 0,
    "uptime_seconds": 3600
  }
}
```

### `get_status` Tool

Quick system state snapshot — useful as a first check before deeper
diagnostics.

**Output includes:** running state, paused state, brightness, FPS
(target/actual), active effect, active profile, layout, device count, total
LEDs.

---

## Tracing & Logging

**Crate:** `tracing` + `tracing_subscriber` with `EnvFilter`
**Source:** `crates/hypercolor-daemon/src/main.rs`

All Hypercolor crates use structured `tracing` for logging. No `println!` in
library code.

### Log Levels

| Level | When to Use |
|-------|-------------|
| `trace` | Extremely verbose: every packet, every frame, every internal state transition |
| `debug` | Detailed diagnostics: protocol packets with hex previews, timing data, state changes |
| `info` | General operations: connections, discovery results, effect changes |
| `warn` | Recoverable issues: performance degradation, retries, fallback behavior |
| `error` | Failures: device errors, protocol violations, unrecoverable states |

### Configuration Priority

1. **`RUST_LOG` environment variable** (highest priority)
2. **`--log-level` CLI flag**
3. **Config file** (`daemon.log_level` in `hypercolor.toml`)
4. **Default:** `info`

### Per-Crate Filtering

The `RUST_LOG` env var supports per-crate granularity:

```bash
# Trace USB transport packets, debug everything else
RUST_LOG=hypercolor_hal::transport=trace,hypercolor=debug just daemon

# Trace only the Razer driver protocol
RUST_LOG=hypercolor_hal::drivers::razer=trace just daemon

# Trace USB backend frame dispatch
RUST_LOG=hypercolor_core::device::usb_backend=trace just daemon

# Trace SMBus backend
RUST_LOG=hypercolor_core::device::smbus_backend=trace just daemon

# Debug discovery orchestrator
RUST_LOG=hypercolor_core::device=debug just daemon

# Trace WebSocket server
RUST_LOG=hypercolor_daemon::api::ws=trace just daemon

# Trace render loop performance
RUST_LOG=hypercolor_daemon::render_thread=trace just daemon

# Multiple targets
RUST_LOG=hypercolor_hal=trace,hypercolor_core::device=debug,hypercolor_daemon::api=debug just daemon
```

### What Gets Logged at Each Level

**`trace` in HAL transports:**
- Every send/receive with full hex preview (first 32 bytes)
- Transport open/close events
- Timeout values and retry attempts

**`debug` in USB backend:**
- Command index within multi-command sequences
- Response parsing results
- CRC validation outcomes
- Connection diagnostics results

**`info` in core:**
- Device discovery results
- Backend connect/disconnect
- Effect start/stop
- Brightness changes

---

## Packet-Level Inspection

**Source:** `format_hex_preview()` in multiple crates

Every transport layer logs packet data as hex when the log level is `debug` or
`trace`. The `format_hex_preview()` utility formats raw bytes for readable log
output.

### Format

```
len=90 bytes=00 1F 00 00 00 03 0F 00 01 00 00 00 00 00 00 0E 01 00 FF 00 00 FF 00 00 ... (+66 bytes)
```

- Shows first N bytes as uppercase hex pairs separated by spaces
- Appends `... (+N bytes)` if truncated
- Returns `<empty>` for zero-length data

### Where Packet Logging Happens

| Transport | File | Preview Size |
|-----------|------|-------------|
| USB Control | `hypercolor-hal/src/transport/control.rs` | 32 bytes |
| USB HID (interrupt) | `hypercolor-hal/src/transport/hid.rs` | 32 bytes |
| USB HIDRAW | `hypercolor-hal/src/transport/hidraw.rs` | 32 bytes |
| USB Bulk | `hypercolor-hal/src/transport/bulk.rs` | 32 bytes |
| USB Backend (core) | `hypercolor-core/src/device/usb_backend.rs` | 32 bytes |
| SMBus Backend (core) | `hypercolor-core/src/device/smbus_backend.rs` | 24 bytes |

### What Gets Logged

**Sends:**
```
DEBUG hypercolor_hal::transport::control: sending control transfer
  packet_hex="00 1F 00 00 00 03 0F 00 01 ..."
```

**Receives:**
```
DEBUG hypercolor_hal::transport::control: received control response
  response_hex="02 1F 00 00 00 03 0F 00 02 ..."
```

**USB command sequences (multi-packet):**
```
DEBUG hypercolor_core::device::usb_backend: usb command bytes
  transport="UsbControl" command_index=0 total_commands=3
  packet_hex="00 1F 00 00 00 03 0F 00 01 00 ..."
```

**Parsed protocol responses:**
```
DEBUG hypercolor_core::device::usb_backend: parsed protocol response
  parsed_data="02 00 03 0F 00 02 00 00 ..."
```

### Example: Trace a Full Razer Init Sequence

```bash
RUST_LOG=hypercolor_hal::transport::control=trace,hypercolor_core::device::usb_backend=debug \
  just daemon
```

This will show every control transfer packet sent during device initialization,
including the init sequence commands, connection diagnostics probes, and the
device's responses.

---

## Protocol Diagnostics Traits

**Source:** `crates/hypercolor-hal/src/protocol.rs`

The `Protocol` trait includes methods specifically designed for diagnostics and
debugging.

### `connection_diagnostics() -> Vec<ProtocolCommand>`

Returns one-shot verification commands sent when a device first connects.
Used to confirm the device is responding correctly.

```rust
// Example: Razer GET_DEVICE_MODE probe
fn connection_diagnostics(&self) -> Vec<ProtocolCommand> {
    vec![ProtocolCommand {
        data: build_get_device_mode_packet(),
        expects_response: true,
    }]
}
```

### `parse_response(data: &[u8]) -> Result<ProtocolResponse, ProtocolError>`

Parses device responses with structured error types:

```rust
pub enum ProtocolError {
    CrcMismatch { expected: u8, actual: u8 },
    MalformedResponse { detail: String },
    DeviceError { status: ResponseStatus },
    EncodingError { detail: String },
}

pub enum ResponseStatus {
    Ok,
    Busy,
    Failed,
    Timeout,
    Unsupported,
}
```

### Transport Error Types

```rust
pub enum TransportError {
    NotFound { detail: String },       // Device not found at expected path
    IoError { detail: String },        // Generic I/O failure
    Timeout { timeout_ms: u64 },       // Response timeout exceeded
    Closed,                            // Transport channel closed
    PermissionDenied { detail: String }, // OS/udev permission issue
    UnsupportedTransfer {              // Wrong transfer type for transport
        transport: String,
        transfer_type: TransferType,
    },
}
```

---

## Mock Backend

**Source:** `crates/hypercolor-core/src/device/mock.rs`

Test the full pipeline without real hardware. The mock backend simulates device
discovery, connection, and frame writing with full call tracking.

### Setup

```rust
use hypercolor_core::device::mock::{MockDeviceBackend, MockDeviceConfig};
use hypercolor_types::spatial::LedTopology;

let backend = MockDeviceBackend::new()
    .with_device(&MockDeviceConfig {
        name: "Test Keyboard".into(),
        led_count: 150,
        topology: LedTopology::Matrix { rows: 6, cols: 25 },
        id: None, // auto-generated
    })
    .with_device(&MockDeviceConfig {
        name: "Test Strip".into(),
        led_count: 60,
        topology: LedTopology::Strip { count: 60 },
        id: None,
    });
```

### Call Tracking

Every method call is recorded for test assertions:

```rust
pub enum MockCall {
    Info,
    Discover,
    Connect(DeviceId),
    Disconnect(DeviceId),
    WriteColors { device_id: DeviceId, led_count: usize },
}

// After operations:
assert_eq!(backend.calls().len(), 3);
assert!(matches!(backend.calls()[0], MockCall::Discover));
```

### Inspection API

```rust
backend.calls()             // &[MockCall] — ordered call log
backend.write_count()       // u64 — total write_colors calls
backend.last_colors(&id)    // Option<&Vec<[u8; 3]>> — last frame data
backend.is_connected(&id)   // bool — connection state
backend.device_infos()      // &[DeviceInfo] — configured devices
```

### Failure Injection

```rust
let mut backend = MockDeviceBackend::new()
    .with_device(&config);

// Simulate connection failures
backend.fail_connect = true;
assert!(backend.connect(&id).await.is_err());

// Simulate write failures
backend.fail_write = true;
assert!(backend.write_colors(&id, &colors).await.is_err());
```

### Mock Transport Scanner

For testing the discovery layer:

```rust
use hypercolor_core::device::mock::MockTransportScanner;

let scanner = MockTransportScanner::new(vec![
    // pre-built DiscoveredDevice instances
]);
```

### Mock Effect Renderer

For testing the effect → color pipeline:

```rust
use hypercolor_core::device::mock::MockEffectRenderer;

let renderer = MockEffectRenderer::new();
// Renders canvas to RGB colors for testing
```

---

## justfile Recipes

**Source:** `justfile` (project root)

### Development & Debugging

```bash
# Run daemon with preview profile and debug logging
just daemon

# Run daemon in release mode (production debugging)
just daemon-release

# Run Servo variant (WebGL-based rendering)
just daemon-servo

# Run CLI with arbitrary args
just cli diagnose --system

# Run all checks (format + lint + test)
just verify

# Run workspace tests
just test

# Test a specific crate
just test-crate hypercolor-hal

# Run a specific test by name
just test-one razer_protocol

# Run clippy
just lint

# Auto-fix clippy suggestions
just lint-fix

# Show dependency tree
just deps

# Count lines of code
just loc
```

### Build Profiles

| Profile | Command | Use Case |
|---------|---------|----------|
| debug | `just build` | Fast iteration, full debug info |
| preview | `just daemon` | Runtime-optimized, debug info preserved |
| release | `just daemon-release` | Full optimization |

---

## udev Setup

**Source:** `udev/99-hypercolor.rules`

USB and I2C device access requires udev rules. Without them, you'll get
`PermissionDenied` errors from transport layers.

```bash
# Install rules and trigger udev reload
just udev-install
```

This copies `99-hypercolor.rules` to `/etc/udev/rules.d/` and triggers
reloading for these subsystems:

- `hidraw` — USB HID raw access
- `usb` — USB device access
- `tty` — Serial port access
- `i2c-dev` — SMBus/I2C access

### Diagnosing Permission Issues

```bash
# Check if udev rules are installed
ls -la /etc/udev/rules.d/99-hypercolor.rules

# Check device permissions
ls -la /dev/hidraw*
ls -la /dev/i2c-*

# Check if your user is in the right groups
groups

# Monitor udev events in real-time
sudo udevadm monitor --subsystem-match=hidraw
```

---

## Common Debug Scenarios

### "Device not detected"

```bash
# 1. Check USB device is visible to the OS
lsusb | grep -i "razer\|corsair\|asus"

# 2. Run the debug discovery tool with trace logging
RUST_LOG=hypercolor_hal=trace,hypercolor_core::device=debug \
  cargo run -p hypercolor-daemon --bin hypercolor-debug -- \
  --log-level trace detect --backends usb,smbus

# 3. Check udev permissions
ls -la /dev/hidraw*

# 4. Check the device database knows this VID:PID
just test-one database_tests
```

### "Device detected but not responding"

```bash
# 1. Trace the transport layer to see packet exchange
RUST_LOG=hypercolor_hal::transport=trace just daemon

# 2. Check connection diagnostics
curl -s -X POST http://localhost:9420/api/v1/diagnose \
  -H "Content-Type: application/json" \
  -d '{"checks": ["devices"]}' | jq .

# 3. Check the output queue state
curl -s http://localhost:9420/api/v1/devices/debug/queues | jq .

# 4. Look for CRC mismatches or protocol errors in logs
RUST_LOG=hypercolor_core::device::usb_backend=debug just daemon 2>&1 | grep -i "crc\|error\|mismatch\|timeout"
```

### "Colors look wrong"

```bash
# 1. Check routing — are zones mapped to the right backends?
curl -s http://localhost:9420/api/v1/devices/debug/routing | jq .

# 2. Monitor frame data via WebSocket
echo '{"type":"subscribe","channels":["frames"],"config":{"frames":{"fps":1,"format":"json"}}}' | \
  websocat ws://localhost:9420/api/v1/ws

# 3. Trace the render pipeline stages
RUST_LOG=hypercolor_daemon::render_thread=trace just daemon
```

### "Frames dropping / low FPS"

```bash
# 1. Subscribe to metrics via WebSocket
echo '{"type":"subscribe","channels":["metrics"],"config":{"metrics":{"interval_ms":500}}}' | \
  websocat ws://localhost:9420/api/v1/ws

# 2. Check per-stage timing breakdown in metrics output
#    Look at: input_sampling_ms, effect_rendering_ms, spatial_sampling_ms, device_output_ms

# 3. Check for backpressure warnings in WebSocket stream

# 4. Identify slow backends
curl -s http://localhost:9420/api/v1/devices/debug/queues | jq .
```

### "New driver development"

```bash
# 1. Start with device discovery to confirm detection
RUST_LOG=hypercolor_core::device=debug \
  cargo run -p hypercolor-daemon --bin hypercolor-debug -- \
  --log-level debug detect --backends usb

# 2. Capture packets from the transport layer at trace level
RUST_LOG=hypercolor_hal::transport=trace just daemon

# 3. Run protocol-level tests
just test-crate hypercolor-hal

# 4. Use mock backend for pipeline integration testing
just test-one mock_integration

# 5. Verify device database entries
just test-one database_tests
```
