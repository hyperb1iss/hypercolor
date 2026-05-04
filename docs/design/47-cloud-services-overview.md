# 47. RFC: Hypercolor Cloud Services Overview

**Status:** Draft. Revised 2026-05-03 after codex cross-model review.
**Date:** 2026-05-03.
**Author:** Bliss (with Nova).
**Depends on:** [09](09-plugin-ecosystem.md), [15](15-community-ecosystem.md), [25](25-distribution-and-applet.md), [46](46-cross-platform-packaging.md).
**Companion specs:** [48](48-hypercolor-remote.md), [49](49-settings-sync.md), [50](50-update-pipeline.md), [51](51-daemon-cloud-connection-protocol.md), [52](52-updater-client.md).

## Summary

Hypercolor is local-first and stays that way. The cloud is opt-in scaffolding around the local daemon: identity, sync, remote access, signed updates, marketplace. Logging in unlocks new capabilities; logging out (or never logging in) leaves the daemon fully functional with every effect, every device, every scene.

The app stays open source in `~/dev/hypercolor`. The hosted cloud service and account dashboard live with the proprietary website in `~/dev/hypercolor.lighting`.

This RFC specifies the umbrella shape: the stack we build on, the deployment topology, the auth contract, the billing model, and the phasing of work. Three companion specs cover the load-bearing subsystems in detail: 48 (Hypercolor Remote), 49 (Settings Sync), 50 (Update Pipeline).

## Goals

1. **Optional accounts.** A user without an account can install Hypercolor, control devices, edit scenes, browse community effects, forever, with no degraded functionality.
2. **Identity-as-multiplier.** Logging in unlocks settings sync, Hypercolor Remote, signed official builds with auto-update, and marketplace publishing.
3. **One Rust backend.** Cloud business logic lives in a single Axum service inside `~/dev/hypercolor.lighting`. The Next.js site owns marketing, auth pages, account tools, and dashboard UX; it does not duplicate cloud business logic.
4. **Better-Auth as the IdP.** Reuse the auth stack already running in `~/dev/v2`. Rust backend verifies JWTs; never duplicates user state.
5. **Free now, billable later.** Stripe is wired from day one with no published Products. Flipping the switch is a runtime config change, not a refactor.
6. **Public client, proprietary service.** Daemon-side cloud code, protocol types, updater, and transport crates live in the open-source app repo. The hosted backend, dashboard, billing, account tooling, release admin, and operational runbooks live in `~/dev/hypercolor.lighting`.
7. **Sub-$10/month at zero users; budget Remote separately.** Hosting baseline (compute, DB, CDN, Workers) stays sub-$10/mo with no Remote traffic. Remote bandwidth is metered linearly and budgeted as `active_remote_users × cap_gb × egress_rate` on top.

## Non-goals

- **Multi-user collaboration.** No shared scenes, no team workspaces, no real-time co-editing. v1 is single-user multi-device.
- **CRDTs.** Settings sync uses server-authoritative ETags. CRDTs are revisited only if real conflicts surface in telemetry. See RFC 49.
- **Proprietary daemon plugin.** The cloud client lives in-tree as a feature-flagged crate, not a separate WASM component or proprietary plugin. WASM packaging is parked for v2 if community pressure demands it.
- **Self-hosted cloud in v1.** The hosted Hypercolor Cloud backend is proprietary. `[cloud].base_url` exists for development, staging, and future compatibility work; it is not a v1 promise that third-party servers can fully replace Hypercolor Cloud.
- **iOS/Android in Phases 1-3.** Native mobile Tauri targets land in Phase 4 alongside signed-build distribution. v1 Remote experience is browser-only on `app.hypercolor.lighting/d/<daemon_id>`. RFC 48 specifies the five client surfaces and their phasing.
- **Custom IdP.** We do not build email verification, password reset, or OAuth provider plumbing in Rust. Better-Auth handles all of that on Next.js.
- **Replacing the marketing site.** `hypercolor.lighting` (Next.js 16, Netlify) stays. The dashboard is also Next.js, on the same site, gated by Better-Auth sessions.

## Stack

| Layer | Pick | Notes |
|---|---|---|
| Backend framework | Axum 0.8 | tower-http, utoipa for OpenAPI, tower-sessions if a Rust portal ever needs sessions |
| Database | Neon Postgres (Launch tier) | Branching for preview DBs, scale-to-zero |
| Persistence | SQLx 0.8 with `query!` macros | `cargo sqlx prepare` checked into `.sqlx/` for offline CI |
| Migrations | `sqlx::migrate!` | Embeds `migrations/` into the binary |
| Compute host | Fly.io single region (ord or sjc) | Persistent WS, scale-to-zero off, ~$2-10/mo |
| Auth | Better-Auth on Next.js (IdP) + JWKS verify in Rust | Device Authorization plugin for daemon/CLI |
| JWT crate | `jsonwebtoken` 9.x | EdDSA for entitlements, RS256/ES256 for Better-Auth verify |
| Billing | `async-stripe` 1.x + `async-stripe-webhook` | Stripe Entitlements API for feature flags |
| Realtime | `axum::extract::ws::WebSocket` + `tokio::sync::broadcast` | Single multiplexed daemon socket per RFC 51 |
| Token storage on client | `keyring` 3.x | OS keychain on macOS/Linux/Windows |
| CDN | Cloudflare R2 + Workers | Zero egress, edge-cached `/v1/updates/check` |
| Marketing site | Next.js 16 on Netlify, same repo | Calls `api.hypercolor.lighting` |
| Dashboard + account tools | Next.js 16 in `~/dev/hypercolor.lighting` | Better-Auth session cookie, calls Rust API |
| Backend repo | `~/dev/hypercolor.lighting` | Proprietary Axum service plus website/dashboard |

Rejected and why:

- **Shuttle.dev:** macro lock-in, more expensive than Fly+Neon at every tier.
- **Cloudflare Workers** for the main backend: no `tokio`, WebSocket coordination requires Durable Objects, wrong shape for this product. Workers gets used only for the edge `/v1/updates/check` endpoint where it shines.
- **Netlify Functions in Rust:** not a real path in 2026. The 2021 WasmEdge integration is unmaintained and lacks WebSocket support.
- **Loco.rs:** young, magic obscures Axum/SeaORM, documented production gotchas.
- **SeaORM / Diesel:** SQLx is the right shape for this domain. Revisit only if relations get heavy.

Alternative stack worth keeping in mind: **Hetzner CX32 (~€7/mo) + Coolify** running the same Axum binary plus a Postgres container, behind Caddy. Flat-cost, infinite headroom for our traffic profile, more babysitting. Decision deferred until Phase 1 lands and we have real ops signal.

## Topology

```
                  hypercolor.lighting (Netlify, Next.js 16)
                  ┌─────────────────────────────────────────┐
                  │ marketing pages (existing)              │
                  │ /dashboard  (new, Better-Auth-gated)    │
                  │ /activate   (device code redirect)      │
                  │ /api/auth/* (Better-Auth)               │
                  │ /api/auth/jwks (public key for Rust)    │
                  └─────────────────────────────────────────┘
                                   │
                                   │ Bearer JWT
                                   ▼
                  api.hypercolor.lighting (Fly.io, Axum)
                  ┌─────────────────────────────────────────┐
                  │ /v1/me/*           account, entitlements│
                  │ /v1/sync/*         REST + WS push       │
                  │ /v1/marketplace/*  browse + install     │
                  │ /v1/relay/*        Hypercolor Remote    │
                  │ /v1/updates/check  signed manifests     │
                  │ /webhooks/stripe   raw body, signed     │
                  └─────────────────────────────────────────┘
                              │                │
                              ▼                ▼
                       Neon Postgres      Cloudflare R2
                       (users, scenes,    (release artifacts,
                        entitlements,      effect bundles,
                        sync_log,          manifest cache)
                        devices)


  Local network                                    Anywhere
  ┌──────────────────────────┐                ┌────────────────────┐
  │ hypercolor-daemon :9420  │ persistent    │ phone / web client │
  │ + reverse WS to /v1/relay│◄── tunnel ────►│ app.hypercolor.    │
  │ + keyring (refresh tok)  │                │ lighting/d/{id}    │
  │ + entitlement JWT cache  │                └────────────────────┘
  └──────────────────────────┘
```

## Repository Shape

New public crates under `~/dev/hypercolor/crates/`:

```
crates/
  hypercolor-cloud-api/      # Pure types: API request/response shapes, error envelopes.
                             #   Zero deps beyond serde+chrono+uuid. Shared contract for daemon,
                             #   updater, UI, and proprietary server.
  hypercolor-cloud-client/   # Daemon-side cloud client. reqwest, OAuth device flow,
                             #   keyring storage, entitlement validation, retry/backoff.
                             #   Behind `cloud` cargo feature in the daemon.
  hypercolor-daemon-link/    # Library: daemon-cloud multiplexed socket protocol, identity,
                             #   channel routing, reconnect, and shared frame shapes.
  hypercolor-updater/        # First-party updater crate, feature-gated out of OSS builds.
```

`hypercolor-daemon` adds a `cloud` feature that pulls in `hypercolor-cloud-client`. OSS builds compile with `--no-default-features` (the `cloud` feature is in the default set for official builds only). The crate boundary keeps the OSS daemon's dependency tree free of `reqwest`, TLS roots, and cloud-specific crates.

Private service code lives under `~/dev/hypercolor.lighting`:

```
hypercolor.lighting/
  src/app/                   # Marketing, dashboard, account UX, activation pages.
  src/server/ or crates/     # Axum cloud API service.
  migrations/                # Postgres schema for sync, devices, entitlements, releases.
  workers/updates-check/     # Cloudflare Worker for edge update checks.
```

## Auth contract

**Boundary:** Better-Auth on Next.js owns identity end-to-end. The Rust backend never duplicates auth state; it only verifies tokens. Daemon and CLI talk **directly to Better-Auth** for device authorization; they never round-trip through Rust for auth operations. Earlier drafts listed Rust `/v1/auth/device/*` endpoints — those are removed.

**Web (marketing site + dashboard).** HttpOnly + Secure + SameSite=Lax session cookie issued by Better-Auth on `hypercolor.lighting`. SSR fetches via `auth.api.getSession({ headers })`. Marketing pages stay public; `/dashboard/*` is gated.

**Daemon.** OAuth 2.0 Device Authorization Grant (RFC 8628). Daemon hits `https://hypercolor.lighting/api/auth/device/code` and `/api/auth/device/token` directly (Better-Auth's Device Authorization plugin endpoints). First run prints a code, opens a browser, polls every 5s. Once approved, daemon receives a short-lived access JWT and a long-lived refresh token. Refresh stored in OS keyring (`keyring` 3.x), key `hypercolor.refresh_token`. Access JWT held in memory and rotated on 401. Logout = revoke refresh token via Better-Auth + clear keyring entry.

**CLI / TUI.** Same device code flow, separate keyring entry (`hypercolor.cli.refresh_token`) so revoke can target the CLI without kicking the daemon. `hypercolor login` is the command.

**Rust backend verification.** Fetches Better-Auth's JWKS at `https://hypercolor.lighting/api/auth/jwks` once on startup, caches with `kid`-based lookup, refreshes on `kid` miss with negative-result throttling. Verifies `Authorization: Bearer <jwt>` on every protected request via `jsonwebtoken`. ~50 LOC.

**JWT validation parameters (canonical, pin in code):**

| Field | Required value |
|---|---|
| `iss` | `https://hypercolor.lighting` |
| `aud` | `hypercolor-daemon` (daemon tokens), `hypercolor-cli` (CLI tokens), `hypercolor-web` (web sessions) |
| `alg` | `EdDSA` (Better-Auth JWT plugin default in 2026) |
| `exp` | required, must be in the future |
| `nbf` | required if present, must be in the past |

A single Rust trait `IdentityProvider { verify_token, fetch_jwks, revoke_device }` abstracts Better-Auth so we can swap to Zitadel or a custom OIDC provider later without touching consumers.

## Daemon identity (per RFC 51)

In addition to the user's bearer JWT, every Hypercolor install holds two pieces of long-lived state generated at first run:

| Item | Generation | Storage |
|---|---|---|
| `daemon_id` | UUIDv4 from `getrandom`, 128 bits, cryptographically random | `~/.config/hypercolor/daemon.toml` plus keyring entry `hypercolor.daemon_id` |
| Daemon Ed25519 identity keypair | `ed25519-dalek::SigningKey::generate(&mut OsRng)` | Private key in keyring `hypercolor.daemon_identity_key`; public key registered with cloud once |

`daemon_id` is **not** derived from hardware. Random IDs are stable across restarts, survive VM/container moves, and avoid binding entitlements to physical hardware. Reinstalls produce a new `daemon_id`; explicit identity migration is a v2 feature.

The identity keypair is used to sign the daemon's WebSocket upgrade per RFC 51, and to authenticate the Hypercolor Remote handshake per RFC 48. A stolen bearer JWT alone cannot impersonate a daemon without the identity private key. RFC 51 owns the registration flow and key rotation UX.

## Billing model

`async-stripe` 1.x in the cloud server, raw-body extractor on `/webhooks/stripe`, signature verification via `stripe-webhook` crate, idempotency via Postgres unique index on `event.id`.

**Entitlements** use Stripe's Entitlements API. Feature keys are **canonical and use the `hc.` prefix everywhere** (RFCs 48, 50, 52 all defer to this schema; earlier drafts that used `"remote"` or `"signed_builds"` bare are wrong):

| Feature key | Meaning |
|---|---|
| `hc.cloud_sync` | Settings sync enabled for this user (RFC 49) |
| `hc.remote` | Hypercolor Remote relay enabled (RFC 48) |
| `hc.signed_builds` | Eligible for signed official builds + auto-update (RFC 50, 52) |
| `hc.marketplace_publish` | Can publish effects to the marketplace |
| `hc.marketplace_paid` | Can purchase paid effects (later) |
| `hc.ai_effects_generate` | Can use AI-generated effects (RFC 53, paid tier) |

Subscription state arrives via the `entitlements.active_entitlement_summary.updated` webhook. Server materializes a row per user in the `entitlements` table. `GET /v1/me/entitlements` returns the canonical entitlement JWT below.

### Canonical entitlement JWT schema

This schema is owned by RFC 47. RFCs 48, 50, 52 reference it; they do not redefine it.

```jsonc
{
  "iss": "https://api.hypercolor.lighting",
  "sub": "<user_id>",
  "aud": ["hypercolor-daemon", "hypercolor-updater", "hypercolor-relay"],
  "iat": 1714780000,
  "exp": 1714783600,             // 1 hour for online checks; see grace section
  "jti": "<ulid>",               // unique per token, used for revocation tracking
  "kid": "ent-2026-01",          // signing key id, allows rotation
  "token_version": 1,
  "device_install_id": "<daemon_id from RFC 51>",
  "tier": "free",                // or "cloud", "team", etc.
  "features": ["hc.cloud_sync", "hc.signed_builds"],
  "channels": ["stable"],        // for hc.signed_builds: which update channels
  "rate_limits": {
    "remote_bandwidth_gb_month": 10,
    "remote_concurrent_tunnels": 5,
    "studio_sessions_month": 5,           // RFC 53 AI Studio
    "studio_max_session_seconds": 30,
    "studio_max_session_tokens": 100000,
    "studio_default_model": "claude-haiku-4-5"
  },
  "update_until": 1746319600     // Unix ts; until when hc.signed_builds is honored offline
}
```

Two TTLs intentionally:

- `exp` is short (1 hour). Online services like the relay tunnel handshake and the entitlement-gated `/v1/updates/check` endpoint require fresh tokens.
- `update_until` is long (1 year typical, matches subscription period). The updater uses this for offline grace per RFC 52 — the daemon may apply staged updates during a 14-day connectivity gap as long as `update_until` is in the future, even if the JWT itself has expired.

**Signing key:** Ed25519, kept in cloud server config as `ENTITLEMENT_SIGNING_KEY`. Rotated annually. Daemon pins the current and previous public keys (rolling window of two) via `kid`.

**Revocation:** `jti` checked against a Postgres deny-list table on each online verification. Stale daemons may use cached entitlements during offline windows but get caught at next online check. Hard revoke = blacklist `jti` and decrement `update_until`.

**Audience scoping:** the `aud` array lets the same JWT serve daemon, updater, and relay roles without minting three separate tokens, while still letting verifiers pin the audience they expect.

**Initial pricing posture.** Free tier ships with everything turned on (`hc.cloud_sync`, `hc.remote`, `hc.signed_builds`). No paid Product is published in Stripe at launch. When pricing flips on:

- Free tier keeps cloud sync and signed builds.
- **"Hypercolor Cloud"** (working title) at ~$5-7/mo gates `hc.remote` plus a future bundle (larger marketplace credits, priority issue triage). Modeled on Nabu Casa's HASS Cloud subscription.
- **"Hypercolor Studio"** (working title) at ~$30/mo bundles `hc.ai_effects_generate` (text-prompt-to-effect via LLM, compile-tested, saved to library) plus everything in Cloud. Designed to be the "this pays for itself" tier for creators. RFC 53 owns the design.
- **"Hypercolor Studio Pro"** at ~$80/mo bundles a higher AI generation budget (60 Sonnet sessions/mo, 300 Haiku/mo, hard $120 provider-spend cap) plus all of Cloud and Studio. For prosumers. RFC 53.
- Paid effects on the marketplace (`hc.marketplace_paid`) are an orthogonal axis.

The numbers are placeholder and decided when we get there. The architecture does not depend on them.

## Cross-cutting concerns

### Privacy

User data the cloud stores: email, hashed password (Better-Auth), display name, settings/scenes/layouts as `jsonb`, device fingerprints, device install records (one per logged-in machine), entitlement state, Stripe customer ID. We do not store device fingerprints we have not been told about; the daemon decides what to push.

Telemetry is **explicitly opt-in**, gated behind a future `hc.telemetry_opt_in` flag in user prefs. The cloud backend ships with telemetry endpoints disabled in v1.

Hypercolor Remote traffic is end-to-end encrypted between the user's browser and the user's daemon. The relay sees TLS frames from the browser and frames over the daemon-WS tunnel; it does not see plaintext. See RFC 48 for the threat model.

### Service Boundary

The Hypercolor Cloud service is proprietary and ships with the website/dashboard. The open-source app repo exposes protocol crates, typed request/response shapes, and feature-gated client code so the local daemon remains auditable and the cloud boundary stays explicit.

Configuration knobs that must exist for hosted ops and local development:

- Database URL.
- Better-Auth issuer URL + JWKS URL.
- Stripe API key (optional, can run without billing).
- R2 / S3 bucket config (optional, for marketplace + updates).
- Signing keys for entitlement JWTs and update manifests (Ed25519 private keys).
- Cloud base URL for daemon/CLI staging and development builds.

### OSS posture

The `cloud` cargo feature is **off by default** in the OSS workspace. `cargo check --workspace` does not pull in `hypercolor-cloud-client`, keyring storage, daemon identity signing, or cloud-specific client code. Official builds enable it via `--features official-cloud` on the daemon and tray crates. Community forkers building from `main` get a fully functional daemon with no cloud client code in the binary.

This is a deliberate departure from the WASM cloud module pattern surveyed in research. We chose feature flags because the only goal WASM uniquely served (third-party swap-in cloud modules) is not a present user demand. Revisit in v2 if the data shifts.

The proprietary backend does not change the license posture of the local engine. Hypercolor stays Apache-2.0; hosted cloud features are first-party services layered around it.

## API surface (high level)

Detailed shapes in companion RFCs. Headline endpoints (note: auth lives entirely in Better-Auth on `hypercolor.lighting/api/auth/*` and is not duplicated here):

```
GET   /v1/me                       # account profile
GET   /v1/me/entitlements          # canonical entitlement JWT (see schema above)
POST  /v1/me/devices               # daemon identity registration (RFC 51)
GET   /v1/me/devices               # list registered installs
DELETE /v1/me/devices/{id}         # revoke an install (kicks tunnel + invalidates JWTs)
DELETE /v1/me                      # account deletion (see Account deletion below)

GET   /v1/sync/scenes              # delta pull (RFC 49)
PUT   /v1/sync/scenes/{id}         # If-Match etag, optimistic write
GET   /v1/sync/layouts
GET   /v1/sync/favorites
GET   /v1/sync/profiles
GET   /v1/sync/changes?since=<n>   # delta pull across all entity kinds

WS    /v1/daemon/connect           # single multiplexed daemon socket (RFC 51)
                                   # carries sync.notifications, relay.http, relay.ws,
                                   # entitlement.refresh, control channels

GET   /d/{daemon_id}/*             # public relay entry; routed to daemon over its
                                   # connection if entitled (RFC 48)

GET   /v1/marketplace/effects      # browse
GET   /v1/marketplace/effects/{id}
POST  /v1/marketplace/effects      # publish (entitlement-gated)
POST  /v1/marketplace/effects/{id}/install  # bind to user library

GET   /v1/updates/check            # signed manifest, edge-cached on Workers (RFC 50, 52)
POST  /webhooks/stripe             # subscription state changes
```

**No more `/v1/sync/ws` and `/v1/relay/connect` as separate endpoints.** Everything daemon-side multiplexes over `/v1/daemon/connect`. RFC 51 owns the wire.

Response envelope matches the existing daemon convention: `{ data: T, meta: { api_version, request_id, timestamp } }`. Errors use Problem Details (RFC 7807): `{ type, title, status, detail, instance }`.

## Phasing

```
Phase 0  (week 0)    RFCs 47/48/49/50 land. 09/15/17 cross-link to cloud touchpoints.
                     Codex review sweep on all four RFCs. Bliss approves shape.

Phase 1  (M1)        crates/hypercolor-cloud-{api,client,server,relay} skeletons.
                     Better-Auth on Next.js dashboard. Device code flow.
                     JWKS verifier on Rust side. Empty Postgres schema with first
                     migration. Fly + Neon deployed under api.hypercolor.lighting.
                     Daemon `--features cloud` builds successfully and can log in.

Phase 2  (M2)        Settings sync (RFC 49). scenes, layouts, favorites, profiles.
                     WS push from server. Daemon-side debounced upload. Schema versioning
                     scaffolding. Conflict UX in tray + TUI.

Phase 3  (M3)        Hypercolor Remote (RFC 48). Reverse WS multiplexing. Browser client
                     at app.hypercolor.lighting/d/{daemon_id}. Free during beta.
                     Marketplace v0: read-only browse, signed-bundle install.

Phase 4  (M4)        Update pipeline (RFC 50/52). Custom hypercolor-updater crate
                     for all surfaces. Entitlement JWT issuance. Code signing CI:
                     Apple Developer ID + Azure Artifact Signing + minisign.
                     R2 bucket for releases. Mobile Tauri targets land here.

Phase 5  (M5+)       Stripe paid tier flips on. Marketplace publishing UX. Paid effects.
                     Telemetry opt-in. Support/admin tooling polish.
```

Phase 0 and 1 are committed; everything after Phase 2 is sequenced but not dated.

## Cost model

Two axes: **baseline** (compute + storage + ops) and **Remote bandwidth** (linear in active Remote users).

### Baseline (no Remote traffic)

| Component | Phase 1 (login only) | 100 cloud-sync users | 1000 cloud-sync users |
|---|---|---|---|
| Fly.io compute | $2 (shared-cpu-1x) | $5-10 (shared-cpu-2x) | $20-40 (2x for HA) |
| Neon Postgres | $0 (Free, scale-to-zero) | $5-15 (Launch) | $25-50 (Scale) |
| Cloudflare Workers + R2 storage | $0 (Free) | $5 (Workers Paid + R2 storage) | $5-10 |
| Apple Developer | $99/yr | $99/yr | $99/yr |
| Azure Artifact Signing | $0 (Phase ≤3) | $9.99/mo (Phase 4+) | $9.99/mo |
| **Baseline run-rate** | **~$2-5/mo** | **~$15-25/mo** | **~$50-100/mo** |
| **One-time + annual fees** | **~$100/yr** | **~$220/yr** | **~$220/yr** |

### Remote bandwidth (additive)

Fly egress is ~$0.02/GB in NA/EU (2026). Cap per free-tier user is 10 GB/mo per RFC 48:

| Active Remote users | Worst-case egress | Egress cost/mo |
|---|---|---|
| 10 | 100 GB | ~$2 |
| 100 | 1 TB | ~$20 |
| 1000 | 10 TB | ~$200 |

Most users will use a tiny fraction of the cap. Realistic expected egress at 100 active users is closer to 100-200 GB/mo, $2-4. The worst-case math above is the budgetary upper bound for capacity planning.

### Operational notes

- **Persistent tunnel actors must not hold Postgres connections.** SQLx pool has explicit max (16 in v1), tunnel actors borrow from the pool only for short bursts (entitlement check, audit log write). Per-tunnel metrics flush to Postgres in 30s batches, not per-frame.
- **Neon scale-to-zero matters.** Phase 1 uses Free tier with scale-to-zero; once persistent WS keeps Neon warm 24/7 (Phase 2+), CU-hours become predictable and we move to Launch.
- **Use Neon's pooled endpoint** for the SQLx connection string to avoid CU-hour explosion under bursty traffic.

Stripe fees are pass-through and only apply once paid tier flips on (~2.9% + $0.30 per transaction in 2026).

## References to companion RFCs

- **RFC 48** specifies Hypercolor Remote: the persistent reverse WebSocket tunnel from daemon to cloud, the multiplexing wire protocol, the auth boundary, and the threat model.
- **RFC 49** specifies Settings Sync: the server-authoritative ETag model, the Postgres schema for synced entities, the WS push protocol, and conflict UX.
- **RFC 50** specifies the server side of the Update Pipeline: signing per-OS, entitlement JWT minting, channels, phased rollout, manifest publishing.
- **RFC 52** specifies the client-side `hypercolor-updater` crate (replacing the earlier draft's Tauri Updater + Velopack split): atomic install per OS, service restart, headless UX, fallback chain.

## Decisions on previously-open questions

The codex review flagged that several "open questions" in the v0 draft were actually decision-blockers. Resolved below.

### Daemon ID

UUIDv4 from cryptographic randomness, generated at first run, persisted in `~/.config/hypercolor/daemon.toml` and OS keyring. Not derived from any hardware identifier. Reinstall produces a fresh ID. RFC 51 owns the lifecycle and the companion identity keypair.

### Domain layout

Three origins, intentionally split for cookie scope and key isolation:

| Origin | Purpose |
|---|---|
| `hypercolor.lighting` | Marketing pages, dashboard at `/dashboard`, Better-Auth IdP at `/api/auth/*`. Session cookies scoped here. |
| `api.hypercolor.lighting` | Rust backend. REST + multiplexed WebSocket. CORS allowlist for `hypercolor.lighting` and `app.hypercolor.lighting`. |
| `app.hypercolor.lighting` | Hypercolor Remote viewer. Browser WASM UI, IndexedDB scoped here for E2E daemon keys (RFC 48). Routes `/d/{daemon_id}/*`. |
| `updates.hypercolor.lighting` | R2 public manifest mirror (RFC 52 fallback). |
| `cdn.hypercolor.lighting` | R2 release artifacts and effect bundles. |

This split prevents a compromise of the Remote viewer origin from touching dashboard cookies or Better-Auth state, and keeps the IndexedDB key store scoped to its actual use.

### Self-hosting posture for v1

No self-hosted cloud backend in v1. The protocol boundary is public and the daemon keeps a configurable `[cloud].base_url` for development, staging, and future compatibility work, but Hypercolor Cloud is a first-party proprietary service shipped from `~/dev/hypercolor.lighting`.

If self-hosting becomes a real demand, v2 gets a separate RFC that decides whether to publish a reference server, a compatibility test suite, or both. Until then, self-hostable local control remains the product guarantee: the daemon works fully without any account.

### Free-tier abuse limits

Codified in the entitlement JWT's `rate_limits` block (see canonical schema above):

- **`remote_bandwidth_gb_month`**: 10 GB/mo on free tier. Tracked per-user, not per-daemon. Reset on the first of each month.
- **`remote_concurrent_tunnels`**: 5 simultaneous active relay tunnels per user. New tunnels beyond this cap evict the oldest.
- **Per-IP rate limit on `/v1/me/devices` (registration)**: 5 per hour. Throttles bot-driven daemon creation.
- **Hard cap of 25 device installs per user.** Above this, registration returns 429 with a "delete an old install first" message.

Usage is surfaced in the dashboard. Paid `hc.remote` tier (Phase 5) raises bandwidth and concurrent caps; specifics deferred to billing flip-on.

### Dashboard and account tools

The website frontend is not only marketing. It owns the human-facing control plane for accounts, devices, subscriptions, and cloud operations.

User-facing dashboard surfaces:

- **Account profile:** display name, email, connected OAuth providers, password/security settings as exposed by Better-Auth.
- **Sessions and tokens:** active browser sessions, daemon/CLI device-code grants, revoke buttons, last-used timestamps.
- **Device installs:** registered `daemon_id`s, install names, OS/arch/version, last seen, Remote status, identity key rotation, revoke install.
- **Entitlements and billing:** active feature flags, Stripe customer/subscription state, bandwidth quota, tunnel count, update channel eligibility.
- **Remote usage:** online/offline state, current tunnel count, monthly transfer, cap warnings, "Remote not enabled" diagnostics.
- **Sync conflicts:** `/dashboard/conflicts` side-by-side resolution for RFC 49 losing versions.
- **Update preferences:** stable/beta/nightly opt-in, skipped versions, frozen-update state, signed-build eligibility.
- **Data export/deletion:** export cloud data, account deletion flow, Stripe/tax-record exception copy.

Admin/support surfaces:

- **User lookup:** account, entitlement, device-install, sync-cursor, and Remote usage inspection.
- **Entitlement overrides:** manual feature grants during beta, revocation, Stripe webhook replay/audit.
- **Release operations:** publish release metadata, adjust cohort caps, pause rollout, recall versions, inspect update metrics.
- **Abuse controls:** disable Remote for a user, clear stuck tunnels, view rate-limit events.
- **Audit log:** sensitive account/admin actions with actor, target, timestamp, IP, and request ID.

### Account deletion

`DELETE /v1/me` is a single endpoint that runs a transactional fan-out:

1. **Authn:** require fresh re-login within last 5 minutes (sensitive operation).
2. **Confirm:** body must include `{ "confirm": "delete-<email>" }` literal.
3. **Postgres:** `DELETE FROM users WHERE id = ?` cascades to entitlements, devices, scenes, layouts, favorites, profiles, sync_log, conflicts, owned_devices, installed_effects (FK ON DELETE CASCADE per RFC 49 schema). Run inside a transaction.
4. **Better-Auth:** call admin API to delete user record; revokes all sessions, refresh tokens, magic links.
5. **Stripe:** cancel active subscriptions, mark customer as `description = "deleted-<deletion_id>-<timestamp>"`. Customer record itself is **kept** because Stripe (and tax law) needs historical transaction records. No new charges possible.
6. **R2:** Phase 5+ when marketplace publishing exists, mark author keys as revoked in marketplace index. Bundles stay (fair-use, attribution) unless user requests removal.
7. **Audit log:** write `account_deletions` row with deletion_id, timestamp, originating IP. Retained 7 years for legal.

A `--dry-run` admin variant fans the same query out without writes for support troubleshooting. GDPR Right of Erasure compliance is met by step 3+4; step 5 is documented as a tax-record exception.

## Future work (deferred, not blocking)

- **AI Effect Generation (RFC 53).** Text-prompt-to-effect. Cloud endpoint generates `effect.html` (canvas2d / WebGL / WGSL) via LLM with system prompt grounded in the Hypercolor SDK and LED color science principles, compile-tests in a headless sandbox, returns the effect plus an iteration handle ("make it slower"). Saved to user library as a generated bundle, signed by the marketplace's `ai-generated` author key. Paid feature gated by `hc.ai_effects_generate`. Free tier may include a small monthly budget for try-before-you-buy. Phase 5+.
- **Marketplace economics.** Stripe Connect (Express vs Standard), revenue split, payout cadence, refunds. Defer to Phase 5 once paid effects flip on.
- **Self-hosting.** A future public compatibility story for third-party cloud servers, if demand appears. v1 is hosted first-party cloud only.
- **Identity migration UX.** "I'm reinstalling, this is the same daemon" flow that preserves daemon_id across reinstall. v2.
- **Multi-region cloud.** Single Fly region in v1. Geographic sharding once latency telemetry warrants.

## Decision log

- **2026-05-03.** Six parallel research swarms validated stack picks. Hybrid (Next.js auth + Rust backend) chosen over single-language alternatives. WASM cloud module deferred to v2. CRDT sync deferred until evidence of conflicts.
- **2026-05-03.** Frontend stays Next.js for marketing AND dashboard. Considered Leptos SSR for dashboard reuse with `hypercolor-ui`; rejected as net-negative complexity. App-side UI (`hypercolor-ui`) stays Rust/Leptos.
- **2026-05-03.** Hypercolor Remote added to scope as a first-class pillar after Nabu Casa parallel surfaced. Folded into Phase 3.
- **2026-05-03 (revision after codex review).** Single multiplexed daemon socket (RFC 51) replaces the earlier draft's `/v1/sync/ws` and `/v1/relay/connect` split. Resolves cross-RFC contradiction.
- **2026-05-03.** Auth boundary: Better-Auth owns identity entirely; Rust backend only verifies JWTs. Removed Rust `/v1/auth/device/*` endpoints from earlier draft.
- **2026-05-03.** Daemon ID = UUIDv4 + Ed25519 identity keypair, both first-run, neither hardware-derived. RFC 51 owns lifecycle.
- **2026-05-03.** Canonical entitlement JWT schema lives in this RFC; RFCs 48/50/52 reference it. Feature keys use `hc.` prefix. Two TTLs (`exp` short for online, `update_until` long for updater grace).
- **2026-05-03.** Cost model split into baseline + Remote bandwidth axes; sub-$10/100-users claim removed; Remote budgeted as `users × cap_gb × egress_rate`.
- **2026-05-03.** Cloud server moved to proprietary `~/dev/hypercolor.lighting`; public repo keeps daemon/client/protocol/updater crates. Self-hosted cloud backend deferred beyond v1.
- **2026-05-03.** Dashboard scope includes account/session/device-install/billing/usage/conflict/release/admin tools, not just marketing pages.
- **2026-05-03.** Account deletion endpoint specified with transactional fan-out across Better-Auth, Postgres, Stripe, R2.
