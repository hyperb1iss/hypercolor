+++
title = "Debugging"
description = "Techniques for diagnosing issues with the daemon, devices, and effects"
weight = 2
template = "page.html"
+++

When things go wrong — devices not responding, effects rendering incorrectly, audio not being captured — here's how to diagnose the problem.

## Daemon Logs

The daemon uses `tracing` for structured logging. Control verbosity with the `RUST_LOG` environment variable or the `--log-level` flag:

```bash
# Maximum verbosity — shows every frame, every USB packet
RUST_LOG=trace just daemon

# Debug logging (recommended for troubleshooting)
RUST_LOG=debug just daemon

# Filter to specific modules
RUST_LOG=hypercolor_hal=debug,hypercolor_core::engine=trace just daemon

# Only show warnings and errors
RUST_LOG=warn just daemon
```

The daemon defaults to `info` level, which logs device connections, effect changes, and errors without flooding the terminal.

### Key Log Targets

| Target | What It Shows |
|---|---|
| `hypercolor_daemon` | API requests, WebSocket connections, startup sequence |
| `hypercolor_core::engine` | Render loop timing, FPS, frame drops |
| `hypercolor_core::effect` | Effect loading, control updates, renderer state |
| `hypercolor_core::input` | Audio capture, FFT processing, beat detection |
| `hypercolor_hal` | Device discovery, USB communication, protocol encoding |
| `hypercolor_hal::drivers::razer` | Razer-specific USB packet details |
| `hypercolor_hal::drivers::prismrgb` | PrismRGB chunked protocol details |

## Built-in Diagnostics

The daemon includes a diagnostic endpoint that runs health checks across all subsystems:

```bash
# Via CLI
hyper diagnose

# Via REST API
curl -X POST http://localhost:9420/api/v1/diagnose | jq
```

This checks:
- Device connectivity and USB permissions
- Audio capture device availability
- Effect engine status
- Configuration validity
- System resource usage

## REST API Inspection

Use `curl` and `jq` to inspect the daemon state:

```bash
# Full system status
curl http://localhost:9420/api/v1/status | jq

# List devices and their status
curl http://localhost:9420/api/v1/devices | jq

# Check the active effect
curl http://localhost:9420/api/v1/effects/active | jq

# View current configuration
curl http://localhost:9420/api/v1/config | jq

# Check audio devices
curl http://localhost:9420/api/v1/audio/devices | jq
```

## WebSocket Monitoring

Monitor the real-time event stream to see what the daemon is doing:

```bash
# Install websocat if needed
cargo install websocat

# Watch all events
websocat ws://localhost:9420/api/v1/ws | jq
```

This shows effect changes, device events, control updates, and errors as they happen. Useful for understanding the timing and sequence of operations.

## USB Debugging

### Permission Issues

If devices are discovered but fail to connect, it's usually a permissions problem:

```bash
# Check if udev rules are installed
ls -la /etc/udev/rules.d/99-hypercolor.rules

# If missing, install them
just udev-install

# Check device permissions directly
ls -la /dev/hidraw*

# See which devices are detected by the kernel
sudo dmesg | grep -i "hid\|usb" | tail -20
```

### Device Not Discovered

If a USB device isn't showing up at all:

```bash
# Check if the device is visible to the system
lsusb

# Get detailed USB descriptor info
lsusb -v -d VENDOR:PRODUCT 2>/dev/null

# Check HID device files
ls /dev/hidraw*

# See udev events in real time, then plug in the device
sudo udevadm monitor --property
```

### Debug Output Queues

The daemon exposes internal device routing state for debugging:

```bash
# See output queue state
curl http://localhost:9420/api/v1/devices/debug/queues | jq

# See device routing table
curl http://localhost:9420/api/v1/devices/debug/routing | jq
```

## Audio Debugging

### No Audio Data

If effects aren't reacting to audio:

1. Check that audio is enabled in config:
   ```bash
   curl "http://localhost:9420/api/v1/config/get?key=audio.enabled" | jq
   ```

2. List available capture devices:
   ```bash
   curl http://localhost:9420/api/v1/audio/devices | jq
   ```

3. Check the audio state:
   ```bash
   # Via MCP tool or REST
   curl http://localhost:9420/api/v1/status | jq '.audio'
   ```

4. Verify your system's audio capture is working:
   ```bash
   # PulseAudio/PipeWire — list sources
   pactl list sources short

   # Check if a monitor source exists
   pactl list sources short | grep monitor
   ```

{% callout(type="tip", title="Monitor sources") %}
For audio-reactive effects, you typically want a "monitor" source that captures your system's audio output (what you hear through your speakers). On PulseAudio/PipeWire, these are named like `alsa_output.*.monitor`.
{% end %}

## Effect Debugging

### Effect Not Loading

```bash
# Rescan the effects directory
curl -X POST http://localhost:9420/api/v1/effects/rescan

# List loaded effects
curl http://localhost:9420/api/v1/effects | jq '.[].name'

# Check daemon logs for loading errors
RUST_LOG=hypercolor_core::effect=debug just daemon
```

### Effect Rendering Issues

Run the daemon with effect engine tracing:

```bash
RUST_LOG=hypercolor_core::effect=trace,hypercolor_core::engine=debug just daemon
```

This shows:
- Which renderer is being used (Servo vs. wgpu)
- Frame timing and potential drops
- Control value injection
- Audio data delivery to the effect

## Common Issues

**"Permission denied" on USB devices**
Install udev rules with `just udev-install` and re-plug the device. You may need to log out and back in for group changes to take effect.

**Daemon starts but no devices connect**
Check `lsusb` to confirm the device is visible. If it is, the vendor/product ID may not be in Hypercolor's device database. Check the logs at `debug` level for discovery output.

**Effects apply but LEDs show wrong colors**
This is usually a spatial layout issue. Check that the device zones are properly positioned in the layout. Try the identify command (`hyper devices identify <id>`) to verify the device is receiving data.

**Low FPS in the render loop**
Enable performance tracing: `RUST_LOG=hypercolor_core::engine=trace`. Look for frame time spikes. Common causes: slow USB writes (try reducing the frame rate), Servo rendering overhead (consider using a wgpu-native effect), or too many devices on the same USB bus.

**WebSocket connection drops**
The daemon pings connected clients periodically. If a client doesn't respond, the connection is dropped. Check your client's WebSocket ping/pong handling. Also verify no reverse proxy or firewall is timing out idle connections.
