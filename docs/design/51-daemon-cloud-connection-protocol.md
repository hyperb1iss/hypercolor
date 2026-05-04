# 51. RFC: Daemon Cloud Connection Protocol

**Status:** Draft. Resolves codex review BLOCKER #1 (single-WS contradiction).
**Date:** 2026-05-03.
**Author:** Bliss (with Nova).
**Depends on:** [47](47-cloud-services-overview.md). **Underlies:** [48](48-hypercolor-remote.md), [49](49-settings-sync.md), [50](50-update-pipeline.md).

## Summary

Hypercolor's daemon maintains exactly **one** persistent WebSocket to the cloud: `wss://api.hypercolor.lighting/v1/daemon/connect`. Every cloud-touching feature multiplexes typed channels over this single socket. Settings sync push, Hypercolor Remote tunneling, entitlement refresh, control plane messages, and event telemetry all share the connection.

Earlier drafts of RFCs 47/48/49 split this into separate sockets (`/v1/sync/ws`, `/v1/relay/connect`) and contradicted the umbrella's "one persistent WS" promise. This RFC owns the canonical wire and lets the feature RFCs reference it.

## Goals

1. **One TCP socket per daemon.** Reduces NAT keepalive cost, simplifies firewall posture, halves connection accounting in the cloud, lets us reason about back-pressure once.
2. **Typed channels with independent entitlement gates.** A user with sync but not Remote sees `relay.*` channels refused at open time, not at TCP connect.
3. **Channel-level back-pressure.** A burst on `relay.ws` (canvas preview frames at 30fps) cannot starve `sync.notifications`.
4. **Authenticated handshake.** The daemon proves identity with both a Better-Auth bearer JWT *and* a long-lived Ed25519 daemon identity key. Establishing the identity key is part of first-run onboarding.
5. **Reconnect-friendly.** Tunnel session resumption is opt-in per channel; sync is stateless across reconnects, Remote tunnels reset on disconnect.

## Non-goals

- **Application-layer state recovery.** This RFC defines the transport. RFCs 48/49 own application semantics (relay session resumption, sync delta cursors).
- **End-to-end encryption between browser and daemon.** RFC 48 owns the E2E layer that runs *inside* the `relay.*` channels. The connection protocol is opaque to E2E payloads.
- **A second connection for high-bandwidth flows.** v1 is one socket. If `relay.ws` canvas preview gets bandwidth-bound for serious users, a sidecar HTTP/2 or QUIC stream is a v2 concern.
- **Cloud endpoint discovery.** Daemon reads cloud base URL from `~/.config/hypercolor/cloud.toml` for official, staging, and development backends. No magic discovery.

## Daemon identity

Two pieces of state per daemon, both established at first run.

| Item | Generation | Storage | Rotation |
|---|---|---|---|
| `daemon_id` | UUIDv4 from `getrandom`, 128 bits, cryptographically random | `~/.config/hypercolor/daemon.toml`, also keyring entry `hypercolor.daemon_id` | Never. Reinstalls produce a new daemon_id; the user can migrate identity in v2. |
| Daemon identity keypair | Ed25519 via `ed25519-dalek::SigningKey::generate(&mut OsRng)` at first run | Private key in OS keyring (`hypercolor.daemon_identity_key`); public key registered with the cloud after first login | Rotatable from the dashboard; old key revoked on rotation. |

`daemon_id` is **not** derived from hardware. No MAC address sniffing, no machine UUID, no CPU serial. Reasoning:

- Hardware fingerprints leak across users when machines change hands.
- Hardware fingerprints are unstable on VMs, containers, dual-boot.
- Random 128-bit IDs collide with probability 2^-122 at one billion daemons; that is fine.
- Hardware fingerprinting would tempt us to bind entitlements to hardware, which we explicitly do not want (a user moving to a new machine should keep their cloud).

`daemon_id` is treated as opaque. The cloud stores it in `device_installations(daemon_id)` and uses it to route relay traffic, scope sync cursors, and key entitlement JWTs. It is not a secret; it appears in URLs (`app.hypercolor.lighting/d/<daemon_id>`). The identity keypair is the secret half.

## Connection lifecycle

### Phase 0: device authorization (RFC 47)

Daemon runs OAuth Device Code flow against Better-Auth, gets a bearer JWT and refresh token, stores refresh in keyring. This phase is unchanged from RFC 47 and does not involve the daemon connection.

### Phase 1: first-run identity registration

After successful login, before opening the persistent socket, the daemon performs **identity registration** exactly once:

```
POST /v1/me/devices
Authorization: Bearer <user-jwt>
Content-Type: application/json

{
  "daemon_id": "01J0...",
  "install_name": "desk-mac",
  "os": "macos",
  "arch": "aarch64",
  "daemon_version": "1.4.2",
  "identity_pubkey": "<base64 Ed25519 public key>",
  "identity_proof": "<base64 Ed25519 signature over daemon_id || '\\n' || identity_pubkey || '\\n' || nonce>",
  "nonce": "<base64 32-byte random>"
}

→ 201 Created
{
  "device": { ... },
  "registration_token": "<short-lived bearer specifically for /v1/daemon/connect>"
}
```

The cloud verifies `identity_proof` against `identity_pubkey`, persists `(daemon_id, identity_pubkey, user_id)` in `device_installations`, and returns a short-lived `registration_token` (audience `daemon-connect`, exp 5 min) that the daemon presents on the upgrade.

**Re-registration** is the same endpoint with the existing daemon_id; cloud verifies the new request is signed by the previously-registered key. Key rotation goes through a dashboard flow that revokes the old pubkey before accepting a new one, breaking any existing tunnel.

### Phase 2: persistent connection

Daemon opens a WebSocket upgrade to:

```
GET wss://api.hypercolor.lighting/v1/daemon/connect
  Authorization: Bearer <registration_token or rotated daemon-bearer>
  Sec-WebSocket-Protocol: hypercolor-daemon.v1
  X-Hypercolor-Daemon-Id: 01J0...
  X-Hypercolor-Daemon-Version: 1.4.2
  X-Hypercolor-Daemon-Ts: <RFC3339 timestamp, fresh within 30s of cloud's clock>
  X-Hypercolor-Daemon-Nonce: <base64 16 bytes, never reused per daemon>
  X-Hypercolor-Daemon-Sig: <Ed25519(canonical_bytes), base64>

where canonical_bytes = SHA256-canonical(
    "GET" || "\n" ||
    "api.hypercolor.lighting" || "\n" ||
    "/v1/daemon/connect" || "\n" ||
    "hypercolor-daemon.v1" || "\n" ||
    daemon_id || "\n" ||
    daemon_version || "\n" ||
    timestamp || "\n" ||
    nonce || "\n" ||
    SHA256(Authorization_jwt_value)
)
```

Cloud verifies in order:

1. JWT validity (Better-Auth JWKS).
2. Timestamp within ±30s of cloud's clock.
3. Nonce not seen before for this `daemon_id` (replay cache, 5-minute TTL keyed on `daemon_id || nonce`).
4. Ed25519 signature over `canonical_bytes` against the registered identity public key.

On any failure, return 401 with a specific `code` in the body (`invalid_jwt`, `clock_skew`, `nonce_replay`, `bad_signature`, `unknown_daemon`). On success returns 101.

The signed upgrade binds the connection attempt to a specific daemon, a specific moment, AND a specific bearer JWT (via the JWT hash in the canonical bytes). A stolen bearer cannot be replayed without also having the identity key. A stolen bearer + identity-signed header cannot be replayed even within the 30s window because the nonce is one-shot.

### Phase 3: hello and channel discovery

First frame each side sends:

```jsonc
// Daemon → Cloud
{
  "kind": "hello",
  "protocol_version": 1,
  "daemon_capabilities": {
    "sync": true,
    "relay": true,
    "entitlement_refresh": true,
    "telemetry": false
  },
  "entitlement_jwt": "<latest cached entitlement>",
  "tunnel_resume": null   // or { session_id, last_seq }
}

// Cloud → Daemon
{
  "kind": "welcome",
  "session_id": "01J...",       // unique per connection
  "available_channels": ["sync.notifications", "control"],
  "denied_channels": [
    { "name": "relay.http", "reason": "entitlement_missing", "feature": "hc.remote" },
    { "name": "relay.ws", "reason": "entitlement_missing", "feature": "hc.remote" }
  ],
  "server_capabilities": { "tunnel_resume": true, "compression": ["zstd"] },
  "heartbeat_interval_s": 25
}
```

The daemon now knows which channels it may open. Denied channels surface in the dashboard ("upgrade to unlock Remote") and the tray.

## Frame format

All frames are JSON over text WebSocket frames, length-bounded by the WebSocket layer. v2 is reserved for length-prefixed binary frames if perf demands.

```jsonc
{
  "channel": "sync.notifications",   // routing
  "kind": "msg" | "open" | "close" | "ack" | "error" | "ping" | "pong" | "hello" | "welcome",
  "msg_id": "01J...",                // ulid, monotonic per channel per direction
  "in_reply_to": "01J...",           // optional, for ack/error
  "payload": { ... },                // channel-specific shape
  "compressed": false                 // true if payload field is base64(zstd(json_bytes))
}
```

`channel`-namespaced messages keep multiplexing trivial: receivers dispatch on `channel`, dispatchers within a channel handle `kind` + `payload`.

## Channels

### Channel admission enforcement

Channel state is one of: **admitted** (cloud allowed it in `welcome`), **denied** (cloud listed it in `welcome.denied_channels`), or **unknown** (any other channel name).

Frame routing rules:

| Frame on | Server action |
|---|---|
| Admitted channel | Forwarded to channel handler. |
| Denied channel | Drop, send `error` frame `{ code: "channel_denied", channel: ..., feature: ... }`. After 3 such frames in 60s, disconnect. |
| Unknown channel | Drop, send `error` frame `{ code: "unknown_channel", channel: ... }`. After 3 such frames in 60s, disconnect. |
| Admitted relay channel without active browser session binding | Drop, send `error` frame `{ code: "no_active_session", channel: "relay.http" }`. |

Admission is **recomputed on entitlement refresh**. When the cloud sends `entitlement.changed` and the new entitlement removes `hc.remote`, the cloud immediately sends `welcome_update` with `denied_channels: ["relay.http", "relay.ws"]` and closes any active relay sessions with reason `entitlement_revoked`.

### `control`

Always open. Cloud sends:

- `entitlement.changed` — daemon should refetch `/v1/me/entitlements`.
- `force.disconnect` — admin-initiated drop, with reason. Daemon backs off.
- `force.relogin` — bearer token revoked; daemon must redo device flow.
- `version.advisory` — minimum supported daemon version, daemon may want to surface "please update."
- `time.skew` — clock difference observed during handshake; daemon adjusts.

Daemon sends:

- `pong` — heartbeat replies.
- `version.report` — periodic posture: daemon_version, uptime, channels in use.

### `sync.notifications`

Owned by RFC 49. Cloud emits `{ kind: "msg", payload: { entity_kind, entity_id, seq } }` advisory pings. Daemon does not push entity content here; REST endpoints remain the source of truth for sync writes and reads.

Subscribed to automatically when channel is admitted. No opt-in payload needed.

### `relay.http` and `relay.ws`

Owned by RFC 48. Cloud forwards browser-initiated HTTP requests as `relay.http` messages and bidirectional browser WebSocket frames as `relay.ws` channel sessions. The actual encrypted envelope lives in the `payload` field; the connection protocol does not look inside.

These channels gate on entitlement `hc.remote`. Without that feature, `welcome.denied_channels` lists them.

### `entitlement.refresh`

Cloud-initiated. When the user's subscription state changes (Stripe webhook), cloud sends `{ kind: "msg", payload: { jwt: "<new_entitlement>" } }` so the daemon doesn't have to wait for the next polling refresh. Daemon caches and applies immediately.

### `studio.preview`

Owned by RFC 53. Carries AI Studio browser-rendered RGBA frames into the daemon as a NEW external render source. Frames are delivered inside the same Remote E2E envelope as `relay.*` (the cloud sees ciphertext only); `studio.preview` is a distinct channel so the daemon can apply RFC 53's stream-to-devices guardrails (brightness ceiling, strobe guard, FPS cap, session timeout, kill switch) before any pixels reach hardware.

Gates on entitlement `hc.ai_effects_generate` AND on the daemon advertising `studio_preview: true` in `hello.daemon_capabilities`. If the daemon does not advertise the capability, the cloud lists `studio.preview` in `welcome.denied_channels` even when the user owns the entitlement.

### `telemetry` (off by default)

Owned by a future RFC. Daemon → cloud one-way telemetry stream, opt-in only. Documented here so the channel name is reserved.

## Heartbeat and timeouts

| Action | Sender | Cadence |
|---|---|---|
| `ping` | Daemon | Every 25s |
| `pong` | Cloud | Reply within 5s |
| `ping` | Cloud | Every 25s, offset 12s from daemon's ping |
| Idle disconnect | Cloud | Drops connection after 75s without any frame from daemon |

Three missed pongs = daemon treats the connection as dead, closes locally, reconnects.

## Reconnect

Daemon-side exponential backoff: 1s, 2s, 4s, 8s, 16s, 32s, 60s ceiling. Jitter ±25%. Daemon never gives up. On every reconnect the daemon repeats Phase 2 (signed upgrade) but skips Phase 1 unless the cloud responds 401 with `code: "registration_revoked"`.

If reconnect arrives within 90s of disconnect and the daemon supplies the previous `session_id` in `tunnel_resume`, the cloud may accept resumption for `relay.*` channels (RFC 48 owns whether to resume or restart). Sync is stateless; resumption is irrelevant there.

## Backpressure and flow control

Per-channel send queues on both sides, bounded:

| Channel | Bound | Drop policy |
|---|---|---|
| `control` | 16 messages | Never drop; if full, disconnect (means something broken) |
| `sync.notifications` | 256 messages | Drop oldest, daemon will full-resync via REST |
| `relay.http` per request | 1 MB outstanding window | Sender pauses |
| `relay.ws` per channel session | 1 MB outstanding window | Sender pauses |
| `entitlement.refresh` | 4 messages | Drop oldest |

Per-tunnel total cap: 8 MB outstanding across all channels. If exceeded, cloud sends `control: { kind: "rate_limit", duration_s: N }` and pauses sending until daemon ACKs. Daemon mirrors.

The application-level backpressure layered on top (per-relay-request 1 MB window) is owned by RFC 48.

## Authentication summary

Three distinct credentials at three layers:

| Credential | Purpose | Storage | Lifetime |
|---|---|---|---|
| Better-Auth refresh token | Renew bearer JWTs | OS keyring | Long (rotated on use) |
| Better-Auth bearer JWT | Authorize REST + WS upgrade | Memory only | 15 min |
| Daemon identity key | Prove this physical daemon is who it claims at upgrade | OS keyring | Long, user-rotatable |

A stolen bearer JWT cannot impersonate a daemon (no identity key). A stolen identity key cannot mint bearers (no refresh token). A stolen pair from one machine reveals scope = exactly that user, and revoking the device install in the dashboard kicks both.

## Implementation

### Crate layout

`crates/hypercolor-cloud-relay/` becomes `crates/hypercolor-daemon-link/` (renamed for clarity).

```
hypercolor-daemon-link/
  src/
    lib.rs                # public API
    handshake.rs          # Phase 2 upgrade canonicalization
    identity.rs           # Ed25519 key material, registration nonce, proof signing
    transport.rs          # tokio-tungstenite wrapper, frame I/O
    multiplex.rs          # channel router, send queues, backpressure
    channels/
      control.rs
      sync.rs             # consumed by hypercolor-cloud-client::sync
      relay.rs            # consumed by hypercolor-cloud-client::relay
      entitlement.rs
    backoff.rs            # reconnect logic
    error.rs
```

The crate is consumed by `hypercolor-cloud-client` in this repo and by the proprietary cloud server in `~/dev/hypercolor.lighting`, since both endpoints implement the same wire.

### Crate dependencies

| Crate | Version | Purpose |
|---|---|---|
| `tokio-tungstenite` | 0.24 | WebSocket I/O |
| `tokio` | 1.x | Runtime |
| `ed25519-dalek` | 2.x | Identity sign/verify |
| `rand_core` / `getrandom` | 0.6 / 0.2 | Ed25519 keygen + registration nonce |
| `keyring` | 3.x | Identity key storage on daemon |
| `uuid` | 1.x | daemon_id |
| `ulid` | 1.x | session_id, msg_id |
| `serde` + `serde_json` | 1.x | Frame encoding |
| `bytes` | 1.x | Buffers |
| `zstd` | 0.13 | Optional payload compression |
| `tracing` | 0.1 | Logging |
| `thiserror` | 2.x | Error enum |

## Open questions

None blocking. The following are nice-to-haves logged as future work:

1. **Binary frame format for v2.** JSON over WS is debuggable but verbose. When `relay.ws` carries 30fps canvas previews, switching to a length-prefixed binary frame (with the same channel routing) saves ~30% on the wire.
2. **QUIC fallback.** Some corporate networks block long-lived WebSockets. QUIC over UDP is more resilient. Out of scope for v1.
3. **Compression negotiation per-channel.** v1 sets compression at the welcome frame; future versions may want compression per-message.

## Decisions

- **2026-05-03.** Single multiplexed socket with typed channels picked over separate sockets per feature. Resolves codex BLOCKER on contradiction across RFCs 47/48/49.
- **2026-05-03.** Daemon ID is UUIDv4 from cryptographic randomness, not hardware-derived. Stable across restarts via keyring + config persistence; new daemon_id on reinstall.
- **2026-05-03.** Daemon identity keypair (Ed25519) generated at first run, registered with cloud once, used to sign every connection upgrade. Solves first-use MITM concern from codex BLOCKER on RFC 48 E2E. Authenticated handshake per RFC 48 layers on top.
- **2026-05-03.** Channel-level entitlement gating happens at `welcome` frame. A free-tier user gets `sync.notifications` admitted and `relay.*` denied with a feature reason the dashboard surfaces.
- **2026-05-03.** No clever resumption in v1. Reconnect = re-handshake + new session_id; relay tunnels reset, sync stays stateless.
- **2026-05-03 (revision after second codex pass).** Upgrade signature now binds nonce + bearer-JWT-hash + canonical method/host/path/protocol bytes. Replay within the 30s timestamp window is blocked by the one-shot nonce cache. Channel admission has explicit enforcement semantics: denied/unknown channels emit `error` frames and disconnect after 3 strikes. Admission is recomputed on entitlement refresh.
