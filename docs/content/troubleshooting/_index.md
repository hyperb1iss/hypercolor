+++
title = "Troubleshooting"
description = "Symptom-first guide: find your failure mode and jump straight to the fix."
weight = 70
sort_by = "weight"
template = "section.html"
+++

Something is not working. Find your symptom in the table below and follow the link — each entry goes directly to the relevant fix, not a general overview.

![Device discovery in the Hypercolor web UI](/img/ui/ui-devices.webp)

## Find your symptom

| Symptom | Where to look |
|---|---|
| Device is plugged in but does not appear in Hypercolor | [Devices not found](@/troubleshooting/devices-not-found.md) |
| Device shows up but will not connect or light up | [Devices not found — connection failures](@/troubleshooting/devices-not-found.md) |
| `Permission denied` on USB device | [Devices not found — udev rules](@/troubleshooting/devices-not-found.md) |
| Another RGB tool (OpenRGB, Aura Sync, openrazer) is holding the device | [Devices not found — conflicting software](@/troubleshooting/devices-not-found.md) |
| Lian Li Uni Hub AL detected but not controlling correctly | [Devices not found — firmware split](@/troubleshooting/devices-not-found.md) |
| PrismRGB Prism 8 shows as "Nollie 8 v2" in the device list | [Devices not found — known rebrands](@/troubleshooting/devices-not-found.md) |
| Audio-reactive effects are static — no reaction to music | [Audio not reacting](@/troubleshooting/audio-not-reacting.md) |
| Microphone is selected instead of system speaker output | [Audio not reacting — monitor source](@/troubleshooting/audio-not-reacting.md) |
| Audio capture shows `audio_available: false` in status | [Audio not reacting — capture pipeline](@/troubleshooting/audio-not-reacting.md) |
| WLED / Hue / Nanoleaf / Govee device not discovered | [Network discovery](@/troubleshooting/network-discovery.md) |
| Govee shows "no devices found" even on the same network | [Network discovery — Govee LAN control](@/troubleshooting/network-discovery.md) |
| Philips Hue link-button pairing fails or times out | [Network discovery — Hue pairing](@/troubleshooting/network-discovery.md) |
| Network device vanishes after a DHCP lease renewal | [Network discovery — stable addressing](@/troubleshooting/network-discovery.md) |
| Effect applies but all LEDs show the same color (no gradient) | [Studio — spatial layout](@/troubleshooting/studio.md) |
| Scene switcher or zone controls behave unexpectedly | [Studio troubleshooting](@/troubleshooting/studio.md) |
| Daemon port 9420 already in use | [Common issues — port conflict](@/troubleshooting/common-issues.md) |
| Daemon fails to start on first login (`XDG_RUNTIME_DIR` not set) | [Common issues — systemd linger](@/troubleshooting/common-issues.md) |
| Effects apply but the web UI preview is dark | [Common issues — WebSocket preview](@/troubleshooting/common-issues.md) |
| Low FPS, stuttering, or render loop lag | [Performance](@/troubleshooting/performance.md) |
| Render loop running but no frames are sent to devices | [Performance — device output queues](@/troubleshooting/performance.md) |
| Servo circuit breaker open — HTML effects frozen | [Performance — Servo circuit breaker](@/troubleshooting/performance.md) |

## Run a quick self-check first

Before digging into a specific page, run the built-in diagnostic command. It checks the render loop, device output queues, and config in about one second:

```bash
hypercolor diagnose
```

Or call the endpoint directly if the CLI is not in scope:

```bash
curl -s -X POST http://localhost:9420/api/v1/diagnose | jq '.data.summary'
```

The response includes a `summary` with `passed`, `warnings`, and `failed` counts and a `checks` array with per-subsystem detail. A warning on `render_loop` or `output_queues` tells you which page to read next.

Target specific check categories to narrow the output:

```bash
# Check only device output state
hypercolor diagnose --check devices

# Check render pipeline only
hypercolor diagnose --check render

# Full report to file — attach to bug reports
hypercolor diagnose --report ~/hypercolor-diag.json --system
```

## Check the logs

When a diagnostic check fails and you need more context, structured logs are the fastest path to the root cause. The daemon uses `tracing` throughout; set `RUST_LOG` before starting it:

```bash
# Recommended starting point for most issues
RUST_LOG=debug just daemon

# USB device problems
RUST_LOG=hypercolor_hal=debug just daemon

# Render pipeline and FPS issues
RUST_LOG=hypercolor_core::engine=trace just daemon

# Audio input
RUST_LOG=hypercolor_core::input=debug just daemon
```

The daemon defaults to `info` level, which is quiet by design. `debug` gives you device connections, protocol steps, and effect changes without flooding the terminal. `trace` on a specific module is only needed when a subsystem is actively misbehaving.

## Troubleshooting pages

{% callout(type="tip") %}
Each page below is organized by symptom, not by subsystem. If your symptom does not appear in the table above, check the page closest to your failure category — the symptoms within each page are more specific than the summary table here.
{% end %}

### [Devices not found](@/troubleshooting/devices-not-found.md)

USB devices that are visible in `lsusb` but do not appear in Hypercolor, permission failures, conflicting software holding the HID device, and firmware-split or rebrand quirks for Lian Li and PrismRGB hardware.

### [Audio not reacting](@/troubleshooting/audio-not-reacting.md)

Audio-reactive effects that stay static: selecting the right PipeWire or PulseAudio monitor source, verifying the audio capture pipeline, and fixing the most common configuration mistakes.

### [Network discovery](@/troubleshooting/network-discovery.md)

WLED, Philips Hue, Nanoleaf, and Govee devices that do not appear in discovery. Covers mDNS requirements, per-protocol pairing flows (Govee LAN control toggle, Hue link-button timing, Nanoleaf power-button hold), VLAN and AP-isolation issues, and firewall rules.

### [Studio](@/troubleshooting/studio.md)

Spatial layout problems (all LEDs showing the same color), zone configuration issues, scene activation not behaving as expected, and layer ordering surprises.

### [Common issues](@/troubleshooting/common-issues.md)

Linux-specific first-run failures that are not hardware or audio related: daemon port conflicts, systemd user session constraints, `XDG_RUNTIME_DIR` not available on headless systems, and the WebSocket preview channel not connecting.

### [Performance](@/troubleshooting/performance.md)

Low FPS, render loop budget misses, device output lag, GPU sampling fallbacks, and Servo circuit breaker trips. Includes how to read the performance snapshot from `hypercolor diagnose` and how to enable per-phase timing traces.

## Still stuck?

If none of the above pages resolve your issue:

1. Run `hypercolor diagnose --report ~/hypercolor-diag.json --system` to capture a full diagnostic snapshot.
2. Check `RUST_LOG=debug` daemon output for the first ERROR or WARN line that appears when the problem occurs.
3. Open an issue on [GitHub](https://github.com/hyperb1iss/hypercolor) and attach the report file.

For developer-level log analysis, internal bus tracing, and WebSocket stream inspection, see [Debugging](@/contributing/debugging.md) in the contributing section.
