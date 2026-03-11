# 26 — Multi-Server Network Discovery & Remote Access

## Overview

Enable Hypercolor daemons to advertise themselves on the local network via
mDNS/DNS-SD, and allow the tray applet and CLI to discover and connect to
any instance — not just localhost. Extend existing security middleware with
network-scoped API key authentication for remote access.

## Goals

1. Daemon publishes `_hypercolor._tcp.local.` mDNS service record on startup
2. Tray applet discovers all Hypercolor instances on the network and lets the
   user switch between them
3. CLI can discover instances via `hyper servers` and connect to any by name/host
4. Remote access works when daemon binds to `0.0.0.0` (opt-in via config)
5. Optional API key protects remote access (integrated with existing tiered auth)
6. All discoverable instances expose a stable identity (name + instance ID)

## Non-Goals

- mTLS or certificate-based auth (future work)
- Multi-server orchestration (applying one effect across multiple daemons)
- Relay/proxy between instances

---

## Architecture

```
┌─────────────────────┐      mDNS publish       ┌──────────────────┐
│  Daemon A            │◄─────────────────────── │  LAN (multicast) │
│  192.168.1.10:9420   │      _hypercolor._tcp   │                  │
│  name: "desk-pc"     │                         │                  │
└─────────────────────┘                          │                  │
                                                 │                  │
┌─────────────────────┐      mDNS publish        │                  │
│  Daemon B            │◄────────────────────────│                  │
│  192.168.1.20:9420   │      _hypercolor._tcp   │                  │
│  name: "server-rack" │                         │                  │
└─────────────────────┘                          └──────────────────┘
                                                        ▲
                                                        │ mDNS browse
                                                        │
                                              ┌─────────┴──────────┐
                                              │  Tray / CLI        │
                                              │  discovers both    │
                                              │  connects to one   │
                                              └────────────────────┘
```

### mDNS Service Record

| Field | Value |
|-------|-------|
| Service type | `_hypercolor._tcp.local.` |
| Instance name | `<instance_name>._hypercolor._tcp.local.` |
| Port | Daemon's HTTP port (default 9420) |
| TXT `id` | Stable UUID v7 instance ID |
| TXT `name` | Human-readable instance name |
| TXT `version` | Daemon version (e.g. `0.1.0`) |
| TXT `api` | API base path (`/api/v1`) |
| TXT `auth` | `none` or `api_key` |

### mDNS Publish Guard

mDNS is only published when the **effective bind address is non-loopback**.
This prevents advertising unreachable instances when the daemon is bound to
`127.0.0.1` (the default). The `mdns_publish` config option is an additional
opt-out switch; it does NOT force publishing on loopback.

| `remote_access` | `listen_address` | Effective bind | mDNS published? |
|-----------------|-------------------|----------------|-----------------|
| `false` | `127.0.0.1` (default) | `127.0.0.1:9420` | No (loopback) |
| `false` | `192.168.1.10` (explicit) | `192.168.1.10:9420` | Yes |
| `true` | `127.0.0.1` (default) | `0.0.0.0:9420` (override) | Yes |
| `true` | `192.168.1.10` (explicit) | `192.168.1.10:9420` | Yes |

Any of the above become "No" if `mdns_publish = false`.

### mDNS Interface Selection

On multi-homed hosts, the mDNS publisher should:
- If the effective bind is `0.0.0.0`, let `mdns-sd` advertise on all interfaces
  (default `ServiceDaemon` behavior)
- If the effective bind is a specific IP, register only that address
- Prefer IPv4 addresses when both IPv4 and IPv6 are available (matching the
  existing WLED scanner behavior at `device/wled/scanner.rs:338`)

---

## Config Changes

### `hypercolor.toml` — new `[network]` section

```toml
[network]
# Publish this daemon via mDNS so other devices can discover it.
# Only takes effect when effective bind is non-loopback.
# Default: true
mdns_publish = true

# Allow connections from other machines (binds to 0.0.0.0 instead of 127.0.0.1).
# Default: false
remote_access = false

# Human-readable name for this instance (shown in discovery).
# Default: system hostname
# instance_name = "desk-pc"
```

> **Note:** `api_key` is NOT stored in `hypercolor.toml`. It is read from the
> `HYPERCOLOR_API_KEY` environment variable (same as existing auth) and managed
> via `SecurityState`. This avoids accidental secret exposure through config API
> endpoints (which allow read-tier access by default).

### `DaemonConfig` additions

```rust
/// Stable instance identifier, auto-generated on first run.
/// Persisted in data dir (not config) to avoid config write-back side effects.
pub instance_id: String,        // loaded from data_dir/instance_id
```

### Instance identity persistence

The instance ID is stored in `data_dir()/instance_id` (e.g.
`~/.local/share/hypercolor/instance_id` on Linux), NOT in the config file.
This avoids:
- Startup write failures on read-only deployments
- Unexpected config file rewrites that clobber user edits
- Schema version bumps for runtime-generated state

On first startup: generate UUID v7, write to data dir. On subsequent startups:
read from data dir. If the file is missing or corrupt, regenerate.

### Behavior matrix

| `remote_access` | `listen_address` | Effective bind |
|-----------------|-------------------|----------------|
| `false` (default) | `127.0.0.1` (default) | `127.0.0.1:9420` |
| `false` | `192.168.1.10` (explicit) | `192.168.1.10:9420` |
| `true` | `127.0.0.1` (default) | `0.0.0.0:9420` (override) |
| `true` | `192.168.1.10` (explicit) | `192.168.1.10:9420` (respected) |

**Warning logic:** Log a warning whenever the effective bind is non-loopback
AND no API key is configured (via `HYPERCOLOR_API_KEY` env var). This covers
both `remote_access = true` and explicit non-loopback `listen_address`:
> "Network-accessible without API key — anyone on your network can control your lights"

---

## API Key Authentication

### Integration with existing `security.rs`

The network API key is added to the **existing** `SecurityState` / `enforce_security`
middleware — NOT a separate auth layer. This preserves the existing rate limiting,
access tier logic, and token extraction pipeline.

**Changes to `SecurityState` / `AuthConfig`:**
- The existing `HYPERCOLOR_API_KEY` env var already populates the control-tier key
- Add `/api/v1/server` to the exempt paths list (alongside existing `/health`)
- No new middleware file needed; extend `security.rs` only

**Accepted token formats** (existing, unchanged):
- Header: `Authorization: Bearer <token>`
- Query parameter: `?token=<value>` (existing name, NOT `?api_key=`)

> **Migration note:** The query parameter remains `?token=` for backward
> compatibility with existing browser preview and WebSocket flows. Do NOT
> introduce `?api_key=` as a second query param name.

**Exempt endpoints** (always accessible, needed for discovery probes):
- `GET /health` (existing)
- `GET /api/v1/server` (new — returns identity only, no control)

**Config endpoint protection:** When auth keys are configured, the config
endpoints (`/api/v1/config*`) must redact the following fields in responses:
- `HYPERCOLOR_API_KEY` / `HYPERCOLOR_READ_API_KEY` values (if surfaced)

> The API key is not stored in `hypercolor.toml`, so the config GET endpoints
> won't expose it. But if env var values are ever surfaced in a debug/status
> endpoint, they must be redacted.

**Rejected requests** use the existing `ApiError::unauthorized()` envelope:
```json
{
  "error": {
    "code": "unauthorized",
    "message": "Valid API key required"
  },
  "meta": { "api_version": "1", "request_id": "req_...", "timestamp": "..." }
}
```

### Client-side

- **CLI:** `--api-key <KEY>` global flag + `HYPERCOLOR_API_KEY` env var (already
  wired for the daemon; CLI reuses the same env var name)
- **Tray:** Stored per-server in a platform-resolved config file

```toml
# <config_dir>/hypercolor/servers.toml (auto-managed by tray)
# Path resolved via paths::config_dir() — not hardcoded ~/.config/
[[servers]]
instance_id = "01912345-6789-7abc-def0-123456789abc"
name = "desk-pc"
api_key = "secret"
```

Platform paths (using existing `hypercolor-core::config::paths`):
- Linux: `~/.config/hypercolor/servers.toml`
- macOS: `~/Library/Application Support/hypercolor/servers.toml`
- Windows: `%APPDATA%\hypercolor\servers.toml`

### Auth hot-reload

API key changes (via env var restart or future config endpoint) require a
**daemon restart** to take effect. The `SecurityState` is constructed once at
router build time (`api/mod.rs:648`). Live reload is future work — document
this clearly in user-facing config docs.

---

## Server Identity in API

### New endpoint: `GET /api/v1/server`

Returns server identity wrapped in the standard `ApiResponse` envelope:

```json
{
  "data": {
    "instance_id": "01912345-6789-7abc-def0-123456789abc",
    "instance_name": "desk-pc",
    "version": "0.1.0",
    "device_count": 3,
    "auth_required": false
  },
  "meta": { "api_version": "1", "request_id": "req_...", "timestamp": "..." }
}
```

> `device_count` is included so discovery probes can show device info without
> requiring a second authenticated request to `/api/v1/devices`.

### WebSocket hello — add `server` field (additive)

New `server` field is additive; existing clients with permissive deserializers
(including the tray applet's `WsHello` struct) will ignore unknown fields.

```json
{
  "type": "hello",
  "server": {
    "instance_id": "01912345-...",
    "instance_name": "desk-pc",
    "version": "0.1.0"
  },
  "state": { ... },
  "capabilities": [ ... ]
}
```

### Status endpoint — add `server` field (additive)

Existing `GET /api/v1/status` response gains an optional `server` object with
the same `ServerIdentity` fields. Additive change — no existing fields removed.

---

## Implementation Plan

### Wave 1: Foundation (types + config)

#### Task 1: Network config types and instance identity

**Files:** `crates/hypercolor-types/src/config.rs`

- Add `NetworkConfig { mdns_publish: bool, remote_access: bool, instance_name: Option<String> }`
- Add `network: NetworkConfig` to `HypercolorConfig` with serde defaults
- `instance_id` is NOT in config — it's loaded at runtime from data dir
- Verify: `cargo check -p hypercolor-types`

#### Task 2: Server identity and discovered server types

**Files:** `crates/hypercolor-types/src/server.rs` (new), `crates/hypercolor-types/src/lib.rs`

- `ServerIdentity { instance_id, instance_name, version }` — for API responses
- `DiscoveredServer { identity: ServerIdentity, host: IpAddr, port: u16, device_count: Option<usize>, auth_required: bool }` — for discovery results
- Serde derives, Clone, Debug
- Verify: `cargo check -p hypercolor-types`

### Wave 2: Daemon (publish + identify + auth)

#### Task 3: mDNS service publisher

**Files:** `crates/hypercolor-daemon/src/mdns.rs` (new), `crates/hypercolor-daemon/src/main.rs`

- `MdnsPublisher` struct wrapping `mdns_sd::ServiceDaemon`
- Register `_hypercolor._tcp.local.` with TXT records on startup
- Unregister on drop (graceful shutdown) — follow existing WLED scanner pattern
  at `crates/hypercolor-core/src/device/wled/scanner.rs:315`
- **Publish guard:** only register if effective bind is non-loopback AND
  `config.network.mdns_publish` is true
- Interface selection: specific IP if bound to one, all interfaces if `0.0.0.0`
- Verify: `cargo check -p hypercolor-daemon`, `dns-sd -B _hypercolor._tcp`

#### Task 4: Instance ID generation and persistence

**Files:** `crates/hypercolor-daemon/src/startup.rs`

- On startup: read `data_dir()/instance_id`; if missing, generate UUID v7 and write
- If `network.instance_name` is `None`, default to system hostname
- Expose `ServerIdentity` via `AppState` for use by API handlers
- Verify: `cargo check -p hypercolor-daemon`

#### Task 5: Remote access bind override

**Files:** `crates/hypercolor-daemon/src/main.rs`

- If `remote_access = true` and `listen_address` is default `127.0.0.1`, override to `0.0.0.0`
- Log warning whenever effective bind is non-loopback and no API key configured
- Verify: `cargo check -p hypercolor-daemon`

#### Task 6: Server identity in API responses

**Files:** `crates/hypercolor-daemon/src/api/ws.rs`, `crates/hypercolor-daemon/src/api/system.rs`

- Add `server: ServerIdentity` to WS `Hello` message (additive field)
- Add `server: ServerIdentity` to status response (additive field)
- Add `GET /api/v1/server` identity endpoint using `ApiResponse` envelope
- Verify: `cargo check`, curl/websocat manual test

#### Task 7: Extend security middleware for network auth

**Files:** `crates/hypercolor-daemon/src/api/security.rs`, `crates/hypercolor-daemon/src/api/mod.rs`

- Add `/api/v1/server` to exempt paths (alongside existing `/health`)
- Existing `HYPERCOLOR_API_KEY` env var and token extraction already handle auth
- No new middleware file — extend `enforce_security` in `security.rs`
- Verify: `cargo check -p hypercolor-daemon`, curl with/without key

### Wave 3: Clients (discover + connect)

#### Task 8: mDNS discovery client

**Files:** `crates/hypercolor-core/src/device/discovery_server.rs` (new)

> Placed alongside existing `device/discovery.rs` (which defines `TransportScanner`,
> `DiscoveredDevice`, `DiscoveryOrchestrator`) to reuse `mdns-sd` patterns. This
> is for discovering Hypercolor **servers** (not devices), so it gets its own file
> but lives in the same module.

- `discover_servers(timeout: Duration) -> Vec<DiscoveredServer>`
- Browse `_hypercolor._tcp.local.`, parse TXT records
- Optionally probe `/api/v1/server` for full identity + device count
- Reuse `mdns-sd` patterns from WLED scanner (`device/wled/scanner.rs`)
- Verify: `cargo test -p hypercolor-core`

#### Task 9: `hyper servers` CLI command

**Files:** `crates/hypercolor-cli/src/commands/servers.rs` (new), CLI registration

- `hyper servers discover` — pretty table of instances (name, host:port, version, devices, auth)
- `hyper servers discover --json` — machine-readable
- `--timeout <secs>` flag (default 3s)
- Add `--api-key` global CLI arg + `HYPERCOLOR_API_KEY` env var support
- Verify: `cargo check -p hypercolor-cli`, manual test

> Named `hyper servers discover` to avoid confusion with existing
> `hyper devices discover` (which scans for LED hardware, not daemon instances).

#### Task 10: Multi-server tray applet

**Files:** `crates/hypercolor-tray/src/daemon.rs`, `crates/hypercolor-tray/src/state.rs`, `crates/hypercolor-tray/src/menu.rs`

- Add `servers: Vec<DiscoveredServer>`, `active_server: Option<usize>` to `AppState`
- Add `TrayCommand::SwitchServer(usize)` and `TrayCommand::RefreshServers`
- **Server discovery trigger:** "Servers" menu item opens a submenu; clicking a
  "Refresh" item in the submenu sends `RefreshServers`. Also scan on startup
  and optionally on a long background timer (10 min).
  > Note: `muda` (tray-icon's menu library) does not fire submenu-opened events,
  > so lazy scan on hover is not feasible. An explicit Refresh action is required.
- "Servers" submenu: list discovered instances, checkmark on active, "Refresh" at bottom
- If only localhost found (or only one server), auto-connect, hide submenu
- **Connection switching:** `SwitchServer` sends a command to `DaemonClient` which
  updates `base_url`/`ws_url`, closes the current WS connection (triggering
  reconnect loop), and re-fetches state from the new server
- API key: read from `<config_dir>/hypercolor/servers.toml` (platform-resolved),
  show "Key required" label if server needs auth and no key is stored
- Verify: `cargo check -p hypercolor-tray`, manual test with multiple daemons

### Wave 4: Tests & Docs

#### Task 11: mDNS round-trip integration test

**Files:** `crates/hypercolor-daemon/tests/mdns_tests.rs`

- Publish a service, discover it, verify fields match
- Test publish guard: loopback bind should NOT publish
- Verify: `cargo test -p hypercolor-daemon mdns`

#### Task 12: Auth middleware tests

**Files:** `crates/hypercolor-daemon/tests/auth_tests.rs`

- Test: no key configured -> all requests pass
- Test: key configured -> missing key returns 401 with `ApiError` envelope
- Test: key configured -> correct key returns 200
- Test: exempt endpoints (`/health`, `/api/v1/server`) pass without key
- Test: `?token=` query param works (NOT `?api_key=`)
- Verify: `cargo test -p hypercolor-daemon auth`

#### Task 13: Update tray state tests

**Files:** `crates/hypercolor-tray/tests/state_tests.rs`

- Test server identity deserialization in WS hello (additive field, backward compat)
- Test server list management and `SwitchServer` command
- Test `servers.toml` loading from platform config dir
- Verify: `cargo test -p hypercolor-tray`

---

## Security Considerations

- **Default safe:** `remote_access = false`, daemon only on localhost
- **mDNS guarded:** only published when effective bind is non-loopback
- **API key is optional but warned about:** loud log message whenever
  non-loopback bind + no key, regardless of how the bind was configured
- **API key NOT in config file:** read from `HYPERCOLOR_API_KEY` env var only,
  so config GET endpoints cannot leak it
- **No key in mDNS:** the `auth` TXT record only says `api_key` or `none`,
  never the actual key
- **Discovery probes are unauthenticated:** `/health` and `/api/v1/server`
  always open so clients can discover instances before authenticating
- **No encryption (HTTP):** acceptable for home LAN; HTTPS is future work
- **Token in query string:** only for WebSocket upgrade (browsers can't set
  headers on WS). Uses existing `?token=` param name. Logged at debug level,
  never at info.
- **Existing auth preserved:** tiered read/control keys, rate limiting, and
  prefix-based tier inference all remain intact

## Future Work

- HTTPS with self-signed certs (auto-generated, trust on first use)
- Multi-instance orchestration (one effect across N daemons)
- mTLS for zero-config secure mesh
- Server groups / rooms in the tray UI
- Live auth config reload (watch `HYPERCOLOR_API_KEY` changes without restart)
- Make key tier explicit in config instead of prefix-based inference
