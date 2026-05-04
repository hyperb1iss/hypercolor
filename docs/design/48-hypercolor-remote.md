# 48. RFC: Hypercolor Remote

**Status:** Draft. Revised 2026-05-03 after codex review (E2E protocol rewritten, wire envelope fixed, transport delegated to RFC 51).
**Date:** 2026-05-03.
**Author:** Bliss (with Nova).
**Depends on:** [47](47-cloud-services-overview.md), [51](51-daemon-cloud-connection-protocol.md).

## Summary

Hypercolor Remote is a persistent reverse WebSocket tunnel from a logged-in user's daemon to the Hypercolor cloud, allowing the user to reach their local daemon's REST and WebSocket APIs from anywhere without VPN, port-forwarding, or dynamic DNS. Modeled on Nabu Casa's Home Assistant Cloud Remote feature: same primitive, RGB-shaped.

The cloud holds an outbound WS connection from each opted-in daemon. When a user hits `https://app.hypercolor.lighting/d/<daemon_id>/api/v1/devices` from their phone browser at the airport, the cloud multiplexes that request over the tunnel, the daemon answers locally, the response streams back. End-to-end encryption protects the payload from the relay.

## Goals

1. **Reach your daemon from anywhere** with no network configuration on the user's side.
2. **Same auth boundary as the local API.** Cloud user sessions (signed in to `hypercolor.lighting`) extend to remote daemons they own. No new auth model; no shared secret per daemon.
3. **Cheap to run at small scale.** A free-tier Hypercolor user with one daemon online costs us cents per month.
4. **End-to-end encryption.** The relay sees ciphertext only. The cloud cannot read scene content, brightness commands, or canvas frames.
5. **Graceful failure.** Daemon offline = browser sees a clear "your daemon is offline" page, not a timeout.

## Non-goals

- **Replacing Tailscale.** We are not a general-purpose VPN. Hypercolor Remote relays Hypercolor protocol traffic, nothing else.
- **Unlimited bandwidth on the free tier.** Phase 5 caps free-tier transfer at 10 GB/month with priority capacity reserved for paid `hc.remote` users.
- **Mesh networking between user's daemons.** A user with two daemons does not get a private mesh; both connect outbound to the cloud. Mesh is a v2+ idea if it earns its keep.
- **Local-network discovery improvements.** Hypercolor's existing mDNS/local discovery is unchanged. Remote handles the case where the client is *not* on the local network.

## Comparison to alternatives

| Approach | Why not |
|---|---|
| **Tailscale tailnet.** Add a tailnet, daemon and client both join, peer-to-peer over WireGuard. | Requires the user to install and configure Tailscale on every client (phone, work browser, etc.). Excellent product, wrong distribution model for "I just want to dim my desk light from work." |
| **Cloudflare Tunnel** (cloudflared). Daemon runs `cloudflared`, Cloudflare exposes a public URL. | Real option, but bakes Cloudflare into the daemon's distribution and gives every user a `*.trycloudflare.com` URL we cannot brand or auth-scope without paying for Cloudflare Access per user. |
| **ngrok / inlets.** Same shape. | Same drawbacks. |
| **Per-daemon STUN/TURN P2P.** Browser dials daemon via WebRTC data channel, TURN fallback. | We already need a relay for "browser is not on the LAN." The TURN fallback path is most of the work; we may as well always relay. |
| **Native VPN client in Hypercolor.** | Months of work, security surface bigger than the rest of the cloud combined, no reason to invent this. |

The reverse-WebSocket relay is the simplest shape that actually solves the user problem. Cloudflare Tunnel and inlets-pro do exactly this internally; we wrap it with our own auth.

## Architecture

### Transport

Hypercolor Remote does not run its own WebSocket. It rides the **`relay.http` and `relay.ws` channels of the multiplexed daemon socket defined in RFC 51**. The cloud routes browser-initiated requests to those channels; the daemon answers locally. Channel admission requires entitlement `hc.remote`; without it, RFC 51's `welcome` frame denies the channels and Remote is not available for that daemon.

This RFC owns:

- The encrypted application envelope sent inside `relay.http` and `relay.ws` channel messages.
- The browser-daemon handshake protocol (key exchange, identity verification, transcript binding).
- The auth boundary at each layer.
- Threat model and operational rules specific to Remote.

RFC 51 owns the wire (frames, channels, heartbeat, reconnect, backpressure between daemon and cloud).

### Three-layer auth, restated

| Layer | Endpoints | Concern |
|---|---|---|
| Tunnel admission (daemon ↔ cloud) | RFC 51 upgrade + `relay.*` channel `welcome` | Daemon proves it owns this `daemon_id` and entitlement covers `hc.remote` |
| User session (browser → cloud) | Better-Auth session cookie on `app.hypercolor.lighting` | User is logged in and owns this daemon |
| End-to-end (browser ↔ daemon) | Encrypted envelope inside `relay.*` channel messages | Cloud cannot read or forge the request |

### End-to-end protocol

The earlier draft passed X25519 public keys through the relay unauthenticated, which made first-use MITM trivial for a compromised cloud. This is replaced.

The daemon already holds a long-lived **identity keypair** (Ed25519) registered with the cloud at first run per RFC 51. The cloud knows the daemon's public identity key and exposes it through a signed lookup:

```
GET https://api.hypercolor.lighting/v1/d/{daemon_id}/identity
→ 200 {
    "daemon_id": "...",
    "identity_pubkey": "<base64 Ed25519>",
    "signed_at": "2026-05-15T10:00:00Z",
    "signature": "<Ed25519 over canonical-JSON, by ENTITLEMENT_SIGNING_KEY>"
  }
```

The cloud signs this record with its own root key (the same `kid`-rolled key that signs entitlement JWTs). The browser verifies the cloud's signature, **then pins** the daemon's identity public key for that daemon_id locally (TOFU on first pairing). On every subsequent connect, the browser verifies the daemon's responses against the pinned key. If the cloud's signed identity record changes after pinning, the browser surfaces "your daemon's identity changed; this could be normal (you reinstalled and rotated keys) or a sign of compromise. Approve or reject."

This converts the trust assumption from "the cloud isn't malicious right now" to "the cloud wasn't malicious at first pairing, OR you intentionally rotated keys." Better but not perfect — see threat model.

#### Handshake (browser-initiated, per session)

A browser session is the period between login on `app.hypercolor.lighting` and logout/timeout. One handshake per session per daemon.

```
1. Browser fetches GET /v1/d/{daemon_id}/identity, verifies cloud signature,
   pins identity_pubkey if first time (or compares to pinned key).

2. Browser generates ephemeral X25519 keypair.

3. Browser sends ClientHello over relay.http channel:
   {
     "kind": "remote.handshake.client_hello",
     "session_id": "<ulid>",
     "protocol_version": 1,
     "browser_session_id": "<stable per-browser ulid, stored in IndexedDB>",
     "ephemeral_x25519_pub": "<base64>",
     "client_nonce": "<base64 32 bytes>",
     "supported_ciphers": ["chacha20poly1305"],
     "claimed_user_id": "<user_id from cookie>"
   }

4. Daemon receives via relay.http. Daemon:
   a. Generates its own ephemeral X25519 keypair.
   b. Computes shared_secret = X25519(daemon_eph_priv, client_eph_pub).
   c. Derives directional keys via HKDF-SHA256:
        salt = client_nonce || daemon_nonce
        info = "hypercolor-remote v1 " || daemon_id || browser_session_id
        c2d_key = HKDF.expand(prk, info || "c2d", 32)
        d2c_key = HKDF.expand(prk, info || "d2c", 32)
   d. Builds ServerHello:
      {
        "kind": "remote.handshake.server_hello",
        "session_id": "<echoed>",
        "protocol_version": 1,
        "ephemeral_x25519_pub": "<base64>",
        "daemon_nonce": "<base64 32 bytes>",
        "selected_cipher": "chacha20poly1305",
        "supported_protocol_versions": [1]
      }
   e. Computes transcript_hash = SHA256(canonical-JSON(ClientHello) || canonical-JSON(ServerHello)).
   f. Signs transcript with daemon identity key:
        identity_sig = Ed25519_sign(identity_priv, "hypercolor-remote-handshake v1 " || transcript_hash)
   g. Sends ServerHello + identity_sig as one frame.

5. Browser receives, derives matching directional keys, verifies identity_sig
   against pinned identity_pubkey over reconstructed transcript_hash.
   If verification fails: abort, surface error to user.

6. Browser sends ClientFinished encrypted with c2d_key:
      payload = canonical-JSON({
        "kind": "remote.handshake.client_finished",
        "session_id": "<echoed>",
        "transcript_hash": "<base64>"
      })
      ciphertext = ChaCha20-Poly1305(c2d_key, nonce_c2d=0, AAD=AAD_template, payload)

7. Daemon decrypts, verifies the transcript_hash matches its own computation.
   Session is now established. seq_c2d=1, seq_d2c=0.
```

Both sides keep `(session_id, c2d_key, d2c_key, seq_c2d, seq_d2c, daemon_id, browser_session_id, protocol_version)` for the session.

#### Steady-state envelope

Every relay frame (request, response, ws frame) carries an encrypted inner envelope. The outer frame structure is owned by RFC 51 (`channel`, `kind`, `msg_id`, `payload`). The Remote `payload` is:

```jsonc
{
  "session_id": "<ulid>",
  "seq": 42,
  "nonce": "<base64 12 bytes>",
  "ciphertext": "<base64>"
}
```

Where:

- `nonce` is `seq` LE-encoded as 12 bytes. **No nonce reuse.** Each direction has its own counter; daemon increments `seq_d2c` for each frame it sends, browser increments `seq_c2d`. ChaCha20-Poly1305 nonce reuse would be catastrophic so the counter is the canonical source.
- `ciphertext = ChaCha20-Poly1305(key, nonce, AAD, plaintext)`, where:
- `AAD = "hypercolor-remote v1 " || protocol_version || session_id || daemon_id || browser_session_id || direction || msg_kind`. AAD binds the frame to its session, direction, and message kind so a frame from one session cannot be replayed in another, and a `request` frame cannot be re-played as a `response`.
- `plaintext` for an HTTP request:

```jsonc
{
  "request_id": "<ulid>",
  "method": "GET",
  "path": "/api/v1/devices",
  "headers": { "user-agent": "..." },   // headers safe to forward to daemon
  "body": "<base64>",                    // empty unless POST/PUT
  "stream_end": true
}
```

- `plaintext` for a WS frame:

```jsonc
{
  "channel_id": "<ulid>",
  "ws_kind": "open" | "frame" | "close",
  "ws_path": "/api/v1/ws",  // open only
  "ws_payload": "<base64>", // frame only
  "ws_binary": true          // frame only
}
```

**Receiver enforces strict monotonic seq.** Out-of-order or duplicate `seq` values are rejected. Replay impossible without breaking the cipher.

**Key rotation.** Re-handshake every 24 hours of session lifetime, or every 2^30 messages, whichever comes first. Re-handshake reuses the existing `browser_session_id` but generates new ephemeral X25519 keys, new directional keys, and resets sequence counters.

#### What stays plaintext (outer routing only)

The relay needs *some* outer metadata to route. Specifically:

- `daemon_id` (URL path).
- `request_id` / `channel_id` for multiplexing on the cloud side. *These are now opaque ULIDs assigned by the browser, not user-meaningful.*
- Frame kind enum (`relay.http` request vs `relay.ws` open/frame/close).
- Frame size, timestamp.

**Specifically removed from plaintext:** HTTP method, path, headers, body, response status, response body. All inside the encrypted inner envelope. Earlier draft listed `authorization` headers in plaintext examples; that was wrong and is removed. The browser **never** forwards its session cookie to the daemon over relay; the daemon authenticates the request as "from this user, via verified Remote session" because the encrypted envelope is end-to-end authenticated, not because it sees a token.

### Backpressure & reconnect

Owned by RFC 51. Remote inherits per-channel 1 MB outstanding windows and the standard heartbeat. On disconnect, all in-flight relay sessions reset (no v1 resumption); browser must re-handshake.

## Auth model

Reflected in the three layers above plus the entitlement gate from RFC 47.

| Layer | Mechanism | Failure mode |
|---|---|---|
| Tunnel admission | RFC 51 upgrade signature + entitlement carries `hc.remote` | 403 at `welcome`; daemon backs off, dashboard surfaces "Remote not enabled" |
| Browser session | Better-Auth cookie on `app.hypercolor.lighting`, ownership check `device_installations.user_id == session.user_id` | 404 (not 403; prevents daemon_id enumeration) |
| End-to-end | Authenticated handshake + ChaCha20-Poly1305 envelope, identity key signed transcript | Browser refuses if pinned daemon identity changes without explicit user approval |

A stolen browser cookie alone cannot decrypt or forge encrypted envelopes (no shared key). A stolen daemon bearer JWT alone cannot impersonate the daemon (no identity key). A fully compromised cloud cannot decrypt the envelope OR forge messages signed by the daemon — but see threat model below for what it CAN do.

## Threat model

| Threat | Mitigation | Honest limit |
|---|---|---|
| Passive cloud operator wants to read user's scenes | E2E encryption: relay sees ciphertext only. Method, path, headers, body all encrypted. | Cloud sees outer routing metadata: `daemon_id`, frame sizes, timing. |
| Active cloud operator wants to send commands to user's daemon | Daemon identity key signs handshake transcript; browser pins on first use. | If user accepts an identity-change prompt without thinking, MITM is possible. |
| Active cloud operator wants to MITM **future** sessions | TOFU pinning means a key swap after first pairing is visible to the browser as an identity change. | First-pairing trust depends on cloud not being malicious at first pairing. There is no perfect defense against this without out-of-band key delivery. |
| Cloud serves malicious browser app code | **Cannot defend.** The cloud delivers the JS/WASM that performs the crypto. A malicious cloud can ship a build that exfiltrates session keys. | The E2E claim covers passive relay/DB compromise, not malicious app delivery. v2 reach goal: subresource integrity + reproducible browser builds + 3rd-party transparency log. |
| Cloud DB breach (read access) | DB stores: emails (Better-Auth), Stripe customer IDs, scene jsonb (RFC 49), entitlement JSON, daemon_id, daemon identity *public* key. | Sync data is plaintext per RFC 49 v1; an operator with DB read can see scenes. Remote payloads are not stored. |
| Cloud full RCE | Attacker can replace browser app, observe outer routing, drop or reorder frames. **Cannot** decrypt past sessions (forward secrecy from ephemeral X25519). **Cannot** forge daemon-signed transcripts. | App-delivery vector remains. |
| Daemon compromised | All bets off for that daemon. | Compromise local; other users unaffected. Rotating the identity key from another machine via dashboard kills the compromised daemon's tunnel. |
| Browser compromised | Per-browser session keys + 24h rotation limit blast radius. | Compromise local to that browser. |
| User's bearer JWT leaks | JWTs short-lived (15 min). Refresh token in keyring/IndexedDB, revocable via dashboard. | If attacker has bearer + identity key, they can impersonate the daemon. Identity key is in OS keyring; high bar. |
| Stolen entitlement JWT | JWT carries `device_install_id`, only valid for the named daemon, validated against revocation list. | 1-hour `exp` limits replay window. |
| Replay attack within session | Strict monotonic per-direction seq counters; receiver rejects duplicates and reordering. | Within window, no impact. |
| Replay across sessions | AAD binds session_id, direction, message kind. Re-encryption from one session to another fails decryption. | None. |
| Downgrade attack on protocol version | Handshake AAD includes `protocol_version`; mismatched versions fail authentication. | None. |
| Downgrade attack on cipher | `selected_cipher` in handshake; v1 supports only `chacha20poly1305`. v2 will negotiate; transcript hash includes the negotiation. | None until v2 introduces second cipher. |
| Cross-daemon replay | AAD includes `daemon_id`; ciphertext from one daemon cannot decrypt as another. | None. |
| DOS via reconnection storm | Per-user concurrent-tunnel cap (5 free), per-IP rate limit on RFC 51 upgrade. | Determined attacker with many IPs can still cause us to scale. Standard cloud problem. |
| DOS via bandwidth | Per-user monthly cap (10 GB free), per-tunnel sustained rate cap. | Cap-buster forces us to suspend the user. |

**Explicitly out of scope:** malicious browser extensions, malicious local OS, nation-state-level attackers with access to Better-Auth's or our root signing keys, side-channel timing on the relay. These are documented limits, not bugs.

**Explicitly honest:** the "cloud cannot read scenes" claim applies to the **Remote** relay path. **Settings sync** in RFC 49 stores scene content as plaintext jsonb in Postgres, which a Hypercolor Cloud operator can read. We do not currently encrypt sync data at rest; that is a v2 work item documented in RFC 49. A user wanting both Remote E2E and end-to-end-encrypted sync must wait for v2. The product copy will be straightforward about this.

## Bandwidth & latency budget

Typical traffic shape:

| Scenario | Bandwidth | Latency tolerance |
|---|---|---|
| Browser polls device list | 5-50 KB/s sporadic | 1s round-trip is fine |
| Browser sends scene activation | 1 KB / event | 200ms round-trip felt |
| Browser views live canvas preview at 30fps | 100-500 KB/s | 200ms latency tolerable, smoothness matters |
| Audio-reactive remote viewing | 100-500 KB/s | Visible jitter at >100ms |

Free-tier monthly cap: 10 GB outbound from the daemon (effectively the user's content download). Most users will use a tiny fraction of this; the cap exists to bound runaway abuse.

Latency target: cloud relay adds <50ms of overhead in steady state on top of the natural Internet RTT. Single-region hosting costs us latency for non-US users; Phase 4+ revisits multi-region if signal demands.

## Operational concerns

### Rate limits

- `/v1/daemon/connect` upgrade: 5 attempts per minute per IP. Throttles reconnection storms.
- Per-user: max 5 concurrent tunnels. Beyond that, oldest tunnel drops.
- Per-tunnel: max 1 MB/s sustained, 5 MB burst. Configurable per entitlement tier.
- Per-user: monthly bandwidth cap. 10 GB free, larger on paid `hc.remote`.

### Monitoring

Cloud emits per-tunnel metrics: connect/disconnect rate, request count, byte count, error rate. Aggregated to per-user counters in Postgres for billing surface. Alerts:

- Tunnel disconnect rate >5% in a 5-min window: page on-call.
- Per-user bandwidth >80% of cap: notify user.
- Per-user concurrent tunnel cap exceeded: silently drop oldest, log.

### Abuse

A user proxying ordinary web traffic through their tunnel would be a misuse but is hard to detect from headers alone. Mitigation: tunnel only accepts Hypercolor API paths (`/api/v1/*`, `/api/v2/*`), rejects everything else with 404 at the cloud layer before forwarding. This keeps the relay a *Hypercolor* relay, not a generic HTTP relay.

### Cost projection

Cloudflare R2 has zero egress, but the relay traffic does not flow through R2. Fly.io egress is $0.02/GB. At free-tier 10 GB/user/mo and 100 active Remote users, that's $20/mo egress. At 1000 users, $200/mo. The paid `hc.remote` tier needs to subsidize this; pricing math runs in RFC 47's billing model.

## Implementation

### Crate layout

`crates/hypercolor-cloud-relay/` contains:

- `protocol/` — frame definitions, serde shapes, codec.
- `client/` — daemon-side tunnel maintainer. Wraps `tokio-tungstenite`. Owns reconnect logic, in-flight request map, per-channel backpressure.
- `server/` — cloud-side hub. Each connected daemon = one `Tunnel` actor. HTTP/WS request from `/d/{daemon_id}/*` finds the tunnel, multiplexes the request, awaits response.
- `crypto/` — X25519 + ChaCha20-Poly1305 helpers via `dalek` + `chacha20poly1305` crates. Browser-side mirror lives in `hypercolor-ui` or a small dedicated TS package.

### Key crate dependencies

| Crate | Version | Purpose |
|---|---|---|
| `tokio-tungstenite` | 0.24 | WebSocket client+server |
| `axum` | 0.8 | Cloud server framework |
| `tokio` | 1.x | Runtime |
| `tower-http` | 0.6 | Tracing, compression |
| `chacha20poly1305` | 0.10 | E2E body encryption |
| `x25519-dalek` | 2.x | Key exchange |
| `serde` + `serde_json` | 1.x | Envelope encoding |
| `bytes` | 1.x | Payload buffers |
| `ulid` | 1.x | Request/channel IDs |
| `prometheus` | 0.13 | Metrics |

### Client surfaces

Hypercolor Remote consumes one Leptos UI codebase (`hypercolor-ui`) across **five client surfaces**, switching transports based on context:

| Surface | Transport | Distribution |
|---|---|---|
| Browser (desktop or mobile web) | Cloud Remote relay | `app.hypercolor.lighting/d/{daemon_id}/`, served as WASM |
| Tauri desktop (macOS / Windows / Linux) | Local LAN to embedded daemon, cloud Remote when away | Code-signed installers, RFC 52 auto-update |
| Tauri mobile (iOS / Android) | Cloud Remote relay primarily; local LAN when on-network | App Store / Google Play |
| Embedded WebView in `hypercolor-desktop` | Local LAN to embedded daemon | Tauri shell |
| Future third-party cloud compatibility | Future public server/API compatibility story | v2+ if demand appears |

This is the load-bearing reuse: same Leptos components, same SilkCircuit tokens, same WebSocket client. The transport layer behind a `RemoteTransport` trait swaps between:

- `LocalTransport` — direct WebSocket to `localhost:9420` (when daemon is on the same machine).
- `LanTransport` — WebSocket to a daemon on the same LAN by mDNS-discovered URL.
- `CloudRelayTransport` — encrypted envelope over RFC 51 / RFC 48 to a remote daemon.

The UI is unaware which it's using; only `RemoteTransport::connect()` differs. Switching mid-session ("I came home, prefer local now") is a future ergonomics improvement, not a v1 requirement.

### Tauri mobile considerations

Tauri 2 supports iOS (WKWebView) and Android (WebView) targets. `hypercolor-ui` already compiles for `wasm32-unknown-unknown`; the Tauri mobile shell wraps that WASM in a native app. Mobile-specific notes:

- **Key storage:** desktop uses `keyring` for the daemon identity key and entitlement cache; mobile uses Tauri's `tauri-plugin-keychain` (iOS Keychain / Android Keystore). Stronger than IndexedDB; preferred where available. Browser remains IndexedDB-only (no choice).
- **Background tunneling:** v1 closes the cloud connection when the app is backgrounded; iOS and Android are aggressive about killing background WebSockets anyway. Push notification on sync events is a v2 work item once an APNs/FCM relay exists.
- **Distribution:** App Store and Google Play distribution for the mobile binaries. Tauri 2 ships their build pipeline; CI matrix expands to include iOS + Android targets in Phase 4+.
- **Offline cache:** mobile users open the app expecting fast paint even on flaky cell. Local cache mirrors the daemon's last seen state from sync (RFC 49); the app shows "last seen 2 minutes ago" while reconnecting.

The mobile target does not require any change to the relay protocol or the E2E handshake. It is "the same Leptos UI in a Tauri shell with mobile-specific key storage and lifecycle hooks." This is the unification the entire `hypercolor-ui` + `hypercolor-leptos-ext` + Cinder thread has been building toward.

## Decisions on previously-open questions

- **Always-encrypt.** The inner request envelope (method, path, headers, body) is always encrypted. Outer routing metadata stays plaintext for the relay to demultiplex. No "sensitive fields" mode. Earlier draft had method/path/headers in plaintext; that is fixed.
- **Per-user bandwidth caps**, not per-daemon. Simpler to communicate, harder to game. Specific limits (10 GB/mo free, 5 concurrent tunnels) live in RFC 47's entitlement schema `rate_limits` block, not duplicated here.
- **WS over SSE.** WS is bidirectional, fits RFC 51's multiplexed channel model.
- **No tunnel session resumption in v1.** Reconnect = re-handshake. v2 may buffer briefly.

## Future work

- **TURN-style direct fallback for same-LAN browser+daemon.** Detect NAT hairpin, attempt local-network bypass via STUN. v2.
- **WebTransport transport.** Once Firefox/Safari ship reliable support. v2.
- **Subresource integrity + reproducible browser app builds.** Mitigate "cloud serves malicious app" threat. v2 reach goal.

## Decision log

- **2026-05-03.** Reverse WS relay chosen over Tailscale, Cloudflare Tunnel, and WebRTC P2P. Reasoning: distribution model (no extra software for the user), branding (we own the URL and the auth surface), and operational fit (we already need persistent daemon-to-cloud WS for sync push).
- **2026-05-03.** End-to-end encryption is in scope for v1, not deferred.
- **2026-05-03.** Single-region (Fly.io) acceptable for v1.
- **2026-05-03 (revision after codex review).** Transport delegated to RFC 51's multiplexed daemon socket. This RFC owns the encrypted application envelope and handshake; RFC 51 owns the wire.
- **2026-05-03.** E2E protocol rewritten with proper authenticated handshake. Daemon identity key (from RFC 51 first-run) signs the handshake transcript; browser pins on TOFU. HKDF directional keys, monotonic per-direction seq counters as nonces, AAD binds session/direction/kind/protocol-version/daemon_id/browser_session_id. ChaCha20-Poly1305.
- **2026-05-03.** Inner request envelope (method/path/headers/body) is always encrypted; only routing metadata stays plaintext. The relay does not see and never forwards browser session cookies.
- **2026-05-03.** Threat model honestly documents the "cloud serves malicious app code" gap. E2E protects against passive relay/DB compromise, not against malicious app delivery.
- **2026-05-03.** Hypercolor Remote is delivered through five client surfaces (browser, Tauri on macOS/Windows/Linux/iOS/Android, embedded WebView), all sharing `hypercolor-ui`. `RemoteTransport` trait abstracts local vs LAN vs cloud relay. Mobile Tauri targets land in Phase 4+ alongside signed-build distribution.
