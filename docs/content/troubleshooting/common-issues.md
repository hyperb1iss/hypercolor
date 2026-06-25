+++
title = "Common issues"
description = "Linux first-run gotchas: port 9420 conflict, systemd-user linger, and XDG_RUNTIME_DIR not set (silent, no error)."
weight = 50
+++

This page covers Linux-specific failures that are not hardware, audio, or network related. They share a common character: the daemon either silently refuses to start, starts with reduced capability, or appears healthy while one subsystem is broken.

Run the built-in self-check first — it catches most of these automatically:

```bash
hypercolor diagnose
```

For devices not showing up at all, see [Devices not found](@/troubleshooting/devices-not-found.md). For audio-reactive effects staying static, see [Audio not reacting](@/troubleshooting/audio-not-reacting.md).

---

## Port 9420 already in use

**Symptom:** The daemon exits immediately with an error containing `failed to bind API server to 127.0.0.1:9420` or `Os error 98 (address already in use)`.

**Why it happens:** Only one process can bind a given address and port. The daemon has a single-instance guard that prints `hypercolor-daemon is already running; exiting` when it fires correctly, but if the previous process died uncleanly — or if something else claimed port 9420 — the TCP bind fails before the guard is relevant.

**Find what is on the port:**

```bash
ss -tlnp | grep 9420
```

Three outcomes:

1. **Another `hypercolor-daemon` process.** A previous instance did not exit cleanly. Stop it:

   ```bash
   hypercolor service stop
   # or directly:
   pkill -x hypercolor-daemon
   ```

   Then start fresh: `hypercolor service start`.

2. **A different process.** Something else owns port 9420. Change Hypercolor's port in `~/.config/hypercolor/hypercolor.toml`:

   ```toml
   [daemon]
   port = 9421
   ```

   Tell the CLI where to reach the daemon — either per-command or exported for the session:

   ```bash
   export HYPERCOLOR_PORT=9421
   ```

3. **Nothing is there.** The port is free but the error appeared anyway. This is rare and usually means a stale Unix socket or a loopback-interface permission issue. Check `hypercolor service status` and review the log:

   ```bash
   hypercolor service logs --lines 50
   ```

**Verify the daemon is accepting connections:**

```bash
curl -i http://localhost:9420/health
```

A `200 OK` with a JSON body (`status`, `version`, `uptime_seconds`, `checks`) means the daemon is up and healthy. A `503` means it is running but a subsystem is degraded. Connection refused means it is not running.

---

## Systemd user service fails on first login

**Symptom:** `hypercolor service status` shows the service as failed or not started after `hypercolor service enable`. Running `hypercolor service start` manually works, but it does not start automatically at login.

**Why it happens:** Hypercolor installs a systemd *user* unit at `~/.config/systemd/user/hypercolor.service`, managed with `systemctl --user`. The unit is ordered after `graphical-session.target`:

```ini
After=graphical-session.target dbus.socket
Wants=graphical-session.target
```

On headless or SSH-only machines `graphical-session.target` is never reached, so the service never starts automatically. A second issue on headless systems: user systemd sessions are created at login and destroyed at logout, so a daemon started via SSH dies when you disconnect.

**Fix for desktop systems (graphical session present):**

The service should start on login. If it does not, confirm it is enabled:

```bash
systemctl --user is-enabled hypercolor
```

If the output is `disabled`:

```bash
hypercolor service enable
# equivalent to:
systemctl --user enable hypercolor
```

**Fix for headless and server systems:**

Enable systemd user linger so the user session persists when no one is logged in:

```bash
loginctl enable-linger "$USER"
```

With linger active the user's systemd instance starts at boot and keeps running indefinitely. The `hypercolor` service can then activate without a graphical session.

Verify linger took effect:

```bash
loginctl show-user "$USER" | grep Linger
# Linger=yes
```

Reload and restart:

```bash
systemctl --user daemon-reload
systemctl --user restart hypercolor
systemctl --user status hypercolor
```

{% callout(type="info") %}
Linger requires systemd 230 or later, which is standard on any distribution shipping systemd since 2016. If `loginctl enable-linger` returns an error, confirm that `systemd-logind` is running: `systemctl status systemd-logind`.
{% end %}

---

## XDG_RUNTIME_DIR not set (silent failure)

**Symptom:** The daemon starts and LEDs light up, but audio-reactive effects stay static, D-Bus integration is absent, or PipeWire screen capture fails. No error is logged. This appears most often on headless or SSH-only machines.

**Why it happens:** `XDG_RUNTIME_DIR` points to a per-user runtime directory (`/run/user/$UID` on most systems) that holds sockets for PipeWire, PulseAudio, D-Bus, and XDG portals. `systemd-logind` creates it when a user session starts and removes it when the last session ends.

When the daemon runs outside an active user session — started via cron, or via linger but before a first interactive login — `XDG_RUNTIME_DIR` may not exist. The daemon treats the variable as optional and starts successfully, but any subsystem that depends on the runtime directory to locate its socket degrades silently.

Specifically affected:

- **Audio.** PipeWire locates its socket at `$XDG_RUNTIME_DIR/pipewire-0` and PulseAudio at `$XDG_RUNTIME_DIR/pulse/native`. Without the directory, audio capture is unavailable and audio-reactive effects are static.
- **D-Bus session bus.** The session bus socket lives at `$XDG_RUNTIME_DIR/bus`. Desktop notifications and MPRIS media-player control will not connect.
- **Screen capture.** `xdg-desktop-portal` — which manages PipeWire screen sharing — depends on the runtime directory.

{% callout(type="warning") %}
The daemon does not log a warning when `XDG_RUNTIME_DIR` is missing. Silent static effects on a server install with no errors in the log is the tell. Check the runtime directory first.
{% end %}

**Diagnose:**

```bash
# Check whether the directory exists
ls /run/user/$(id -u)/

# Check for audio sockets
ls /run/user/$(id -u)/pipewire-0 2>/dev/null || echo "PipeWire socket missing"
ls /run/user/$(id -u)/pulse/native 2>/dev/null || echo "PulseAudio socket missing"

# Check what the daemon sees
hypercolor audio devices
```

**Fix on desktop systems:**

Log out and log back in. `systemd-logind` recreates the runtime directory and restarts the user services that populate it. If the problem persists after a fresh login, check that `systemd-logind` is running:

```bash
systemctl status systemd-logind
```

**Fix on headless systems:**

Enable linger first (see [Systemd user service fails on first login](#systemd-user-service-fails-on-first-login) above). With linger active, `systemd-logind` maintains `XDG_RUNTIME_DIR` even without an active session.

If audio or D-Bus integration is required on a headless machine, enable socket activation for those services explicitly:

```bash
systemctl --user enable --now pipewire.socket pipewire-pulse.socket
```

For audio-reactive effects on a headless server you may also need a virtual sink or a hardware audio device accessible to the daemon's user. See your distribution's documentation for headless PipeWire or PulseAudio setup.

---

## WebSocket preview not connecting

**Symptom:** The daemon is running and devices are lit up, but the web UI canvas preview shows a black rectangle or the TUI spectrum display is empty. The effect is applying to LEDs correctly.

**Why it happens:** The canvas preview and spectrum data travel over a WebSocket connection to `/api/v1/ws` on port 9420. The preview channel is separate from the LED output pipeline — devices can light up even when the WebSocket handshake fails.

Common causes:

- **Browser extension or proxy blocking the upgrade.** Ad blockers and privacy extensions sometimes intercept WebSocket connections. Try disabling extensions or opening the UI in a private window.

- **Reverse proxy not forwarding WebSocket.** If Hypercolor is behind nginx or Caddy for remote access, the proxy must forward the `Upgrade` header. A misconfigured proxy returns `400 Bad Request` on the handshake, leaving the preview dark while the REST API works.

- **CORS blocking the browser.** Loopback origins (`localhost` and `127.0.0.1`, on any port) are always trusted, so a local UI never hits this. It only bites when you reach the daemon from another host on a daemon that has API auth enabled: the browser refuses the WebSocket upgrade from an untrusted origin. Add that remote origin to `cors_origins` in `~/.config/hypercolor/hypercolor.toml`:

  ```toml
  [web]
  cors_origins = ["https://lights.example.com"]
  ```

**Verify the WebSocket endpoint directly:**

```bash
# requires: cargo install websocat
websocat ws://localhost:9420/api/v1/ws
```

You should see a stream of JSON events. If the connection is refused or immediately closed, check the daemon log:

```bash
hypercolor service logs --lines 50
```

If events arrive in `websocat` but the browser preview is still dark, the problem is on the browser side. Open the developer console and look for WebSocket errors in the Network tab.
