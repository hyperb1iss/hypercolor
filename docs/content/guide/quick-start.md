+++
title = "Quick Start"
description = "Get from zero to RGB in five minutes"
weight = 2
template = "page.html"
+++

This guide assumes you've completed the [installation](@/guide/installation.md) and have the daemon running on `localhost:9420`.

## 1. Start the Daemon

If it's not already running:

```bash
just daemon
```

{% callout(type="tip", title="Daemon + UI together") %}
Use `just dev` to launch the daemon and the web UI dev server simultaneously. The UI will be available at `http://localhost:9430` with hot reload.
{% end %}

## 2. Check Device Discovery

Hypercolor automatically discovers connected USB HID devices and network devices (WLED via mDNS). Check what's been found:

**Via CLI:**

```bash
hypercolor devices list
```

**Via REST API:**

```bash
curl http://localhost:9420/api/v1/devices | jq
```

You should see a response envelope with `data.items`, the discovered devices with their names, types, LED counts, and connection status.

{% callout(type="info", title="No hardware yet?") %}
You can still browse and preview effects through the web UI without physical devices connected. The preview renderer shows what effects look like on a virtual canvas.
{% end %}

## 3. Browse Effects

Hypercolor ships with a library of 30+ built-in effects: audio-reactive visualizers, ambient gradients, particle systems, and more.

**Via CLI:**

```bash
hypercolor effects list
```

**Via REST API:**

```bash
curl http://localhost:9420/api/v1/effects | jq '.data.items[].name'
```

**Via Web UI:**

Open `http://localhost:9430` (if running `just dev`) or `http://localhost:9420` (if the daemon is serving the embedded UI). The effect browser lets you search, filter by category, and preview effects before applying.

## 4. Apply an Effect

Pick an effect ID from the list and apply it:

**Via CLI:**

```bash
hypercolor effects activate <effect-id>
```

**Via REST API:**

```bash
curl -X POST http://localhost:9420/api/v1/effects/<effect-id>/apply
```

You can also apply effects with custom control values:

```bash
curl -X POST http://localhost:9420/api/v1/effects/<effect-id>/apply \
  -H "Content-Type: application/json" \
  -d '{"controls": {"speed": 7, "intensity": 85}}'
```

## 5. Adjust Controls

Most effects expose user-configurable parameters — speed, color palette, intensity, audio reactivity. Tweak them in real time:

**Via REST API:**

```bash
curl -X PATCH http://localhost:9420/api/v1/effects/current/controls \
  -H "Content-Type: application/json" \
  -d '{"controls": {"speed": 3, "palette": "SilkCircuit"}}'
```

Changes apply immediately. The render loop picks up new control values on the next frame.

## 6. Connect via WebSocket

For real-time state updates, connect to the WebSocket endpoint:

```bash
websocat ws://localhost:9420/api/v1/ws
```

You'll receive JSON messages whenever the system state changes: effects applied, devices connected/disconnected, control values updated. The web UI and TUI both use this channel for live updates.

## What's Next

- [Configuration](@/guide/configuration.md) — Set up profiles, audio input, and device mappings
- [Creating Effects](@/effects/creating-effects.md) — Write your own effects with the TypeScript SDK
- [REST API Reference](@/api/rest.md) — Full API documentation
- [Hardware Support](@/hardware/_index.md) — Details on supported devices and drivers
