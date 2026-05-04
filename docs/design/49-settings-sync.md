# 49. RFC: Settings Sync

**Status:** Draft. Revised 2026-05-03 after codex review.
**Date:** 2026-05-03.
**Author:** Bliss (with Nova).
**Depends on:** [47](47-cloud-services-overview.md), [51](51-daemon-cloud-connection-protocol.md).

## Summary

Settings sync replicates a logged-in user's scenes, layouts, favorites, profiles, and preferences across multiple daemons (desktop, laptop, work machine). It is single-user multi-device fan-out, not collaborative editing.

The design is server-authoritative with per-row ETags, optimistic local writes, and WebSocket push for low-latency convergence. CRDTs are explicitly deferred. The schema and conflict UX are designed to make a future CRDT migration on a per-entity basis straightforward if telemetry shows real conflicts.

## Goals

1. **A user with multiple Hypercolor installs sees the same scenes, layouts, favorites, and profiles on every install.**
2. **Convergence in <2 seconds** when both daemons are online.
3. **Offline edits don't silently lose data.** Conflicts surface explicitly, never get clobbered.
4. **Cheap.** A free-tier user with 10 scenes and 5 layouts costs us cents per month in storage and writes.
5. **Schema-evolvable.** New fields land without breaking older clients.
6. **Honest about device locality.** Things that are local-only (current `DeviceId`, transient render state, audio device pick) never sync.

## Non-goals

- **Real-time co-editing.** Two users editing the same scene at once is not a v1 use case.
- **Selective sync.** v1 syncs everything sync-eligible. "Don't sync this scene" is a v2 feature.
- **Syncing custom effect bundles' bytes.** We sync metadata and source URLs; the actual `.html` files come from the marketplace or community URLs.
- **Backup-as-a-feature.** Sync is convergence, not archival. A separate "export everything" endpoint is sufficient for backup needs.
- **CRDTs in v1.** See "Sync model rationale" below.

## What syncs and what doesn't

| Entity | Syncs? | Why |
|---|---|---|
| User preferences (theme, brightness defaults, opt-ins) | Yes, LWW per-key | Small scalars, conflicts are losses no one notices |
| Saved scenes (named groupings + effect assignments) | Yes, per-row ETag | The flagship sync target |
| Spatial layouts (device positions in a room) | Yes, per-row ETag | Per-fingerprint device positions |
| Favorites (effect IDs + control overrides + position) | Yes, per-row ETag | Personal effect library |
| Profiles (full app state snapshots) | Yes, per-row ETag | "Save and restore everything" |
| Installed effect bundles | Metadata only | bundle_id, source_url, version. The bytes live in the marketplace. |
| Device library (devices the user owns) | By fingerprint, not local DeviceId | Vendor + model + serial → user_owns_this_device. Each install resolves to local DeviceId at attach time. |
| Per-device profile (custom name, default zones, etc.) | Yes, keyed by device fingerprint | Survives reinstalls, survives moves between machines |
| Current scene activation state | No | Local. Each install runs its own scene. |
| Active effect, current brightness | No | Transient. Not interesting to replicate. |
| Audio device pick | No | Hardware-local. |
| Render canvas dimensions | No | Resolution-dependent local config. |
| Window position, last-focused tab | No | Workstation-local UI state. |
| Stripe customer ID, entitlements | Server-owned | Daemon reads, never writes. |

The split rule: **if it would be wrong on a different machine, don't sync it.**

## Sync model rationale

### Considered

| Model | Verdict |
|---|---|
| Last-write-wins with timestamps | Loses concurrent edits silently. Fine for scalars only. |
| **Server-authoritative with per-row ETags** | **Picked.** Free per-field conflict detection, no metadata bloat, easy schema migration, queryable as SQL. |
| Event-sourced (append-only ops log, fold to state) | Storage grows forever or needs compaction. Overkill for "user dimmed the desk light." |
| CRDT (Loro / Automerge 3 / yrs) | Solves a problem we don't have (concurrent same-doc human editing). Costs: per-field IDs, tombstones, opaque blobs, painful schema migrations. |
| Hybrid (CRDT for collections, ETag for scalars) | The right shape *if* we had a real CRDT need. We don't, yet. |

### Reference points

- **Obsidian Sync:** three-way diff for markdown, LWW for binaries, **JSON-key-merge for settings.** That last bit is directly applicable; we mirror it.
- **Linear:** server-authoritative with optimistic local mutations, sync engine over WebSocket. Same shape as ours.
- **1Password / Bitwarden:** server-authoritative, encrypted blobs per item, LWW per item.
- **Tailscale:** policy is a single source-of-truth file; pure server-authoritative. Closest analog to our model.

The pattern is overwhelming: **single-user multi-device sync is server-authoritative with per-record ETags.** CRDTs only show up when the same blob has multiple concurrent human authors.

### When we revisit CRDTs

If telemetry from Phase 2 shows >1% of users hitting an unresolvable conflict on the same entity, the most likely candidate for CRDT migration is **layouts** (concurrent device-position editing on one machine while another names devices in the same layout). Loro 1.0 with a `Map<device_fingerprint, {x, y, z, name}>` would merge cleanly. v1 ships per-row ETag; v2 swaps in Loro for layouts only if data warrants it.

## Schema

```sql
-- Users come from Better-Auth's users table; we reference by id.
-- Schema lives in `~/dev/hypercolor.lighting` migrations for the proprietary
-- cloud server.

CREATE TABLE device_installations (
    id              UUID PRIMARY KEY,
    user_id         UUID NOT NULL REFERENCES auth.users(id) ON DELETE CASCADE,
    daemon_id       UUID NOT NULL,             -- self-generated UUID, stable per install
    install_name    TEXT NOT NULL,             -- user-chosen ("desk-mac", "laptop")
    os              TEXT NOT NULL,             -- macos, linux, windows
    arch            TEXT NOT NULL,             -- aarch64, x86_64
    daemon_version  TEXT NOT NULL,
    last_seen_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (user_id, daemon_id)
);

CREATE TABLE prefs (
    user_id     UUID NOT NULL REFERENCES auth.users(id) ON DELETE CASCADE,
    key         TEXT NOT NULL,
    value_json  JSONB NOT NULL,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_by  UUID REFERENCES device_installations(id),
    PRIMARY KEY (user_id, key)
);

CREATE TABLE scenes (
    id           UUID PRIMARY KEY,
    user_id      UUID NOT NULL REFERENCES auth.users(id) ON DELETE CASCADE,
    name         TEXT NOT NULL,
    definition   JSONB NOT NULL,            -- full scene blob, see Definition shapes below
    schema_version INT NOT NULL DEFAULT 1,
    etag         BIGINT NOT NULL DEFAULT 1,  -- monotonic per-row counter
    deleted_at   TIMESTAMPTZ,                -- soft-delete tombstone
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX scenes_by_user ON scenes(user_id, updated_at);
CREATE INDEX scenes_undeleted ON scenes(user_id, updated_at) WHERE deleted_at IS NULL;

CREATE TABLE layouts (
    id           UUID PRIMARY KEY,
    user_id      UUID NOT NULL REFERENCES auth.users(id) ON DELETE CASCADE,
    name         TEXT NOT NULL,
    definition   JSONB NOT NULL,
    schema_version INT NOT NULL DEFAULT 1,
    etag         BIGINT NOT NULL DEFAULT 1,
    deleted_at   TIMESTAMPTZ,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX layouts_by_user ON layouts(user_id, updated_at);
CREATE INDEX layouts_undeleted ON layouts(user_id, updated_at) WHERE deleted_at IS NULL;

CREATE TABLE favorites (
    user_id      UUID NOT NULL REFERENCES auth.users(id) ON DELETE CASCADE,
    effect_id    TEXT NOT NULL,
    controls     JSONB,
    position     INT,
    etag         BIGINT NOT NULL DEFAULT 1,
    deleted_at   TIMESTAMPTZ,
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (user_id, effect_id)
);
CREATE INDEX favorites_by_user ON favorites(user_id, updated_at);
CREATE INDEX favorites_undeleted ON favorites(user_id, updated_at) WHERE deleted_at IS NULL;

CREATE TABLE profiles (
    id           UUID PRIMARY KEY,
    user_id      UUID NOT NULL REFERENCES auth.users(id) ON DELETE CASCADE,
    name         TEXT NOT NULL,
    snapshot     JSONB NOT NULL,
    schema_version INT NOT NULL DEFAULT 1,
    etag         BIGINT NOT NULL DEFAULT 1,
    deleted_at   TIMESTAMPTZ,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX profiles_by_user ON profiles(user_id, updated_at);
CREATE INDEX profiles_undeleted ON profiles(user_id, updated_at) WHERE deleted_at IS NULL;

CREATE TABLE owned_devices (
    user_id      UUID NOT NULL REFERENCES auth.users(id) ON DELETE CASCADE,
    fingerprint  TEXT NOT NULL,        -- vendor:model:serial or vendor:model:mac
    custom_name  TEXT,
    metadata     JSONB,
    etag         BIGINT NOT NULL DEFAULT 1,
    deleted_at   TIMESTAMPTZ,
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (user_id, fingerprint)
);
CREATE INDEX owned_devices_by_user ON owned_devices(user_id, updated_at);
CREATE INDEX owned_devices_undeleted ON owned_devices(user_id, updated_at) WHERE deleted_at IS NULL;

CREATE TABLE installed_effects (
    user_id      UUID NOT NULL REFERENCES auth.users(id) ON DELETE CASCADE,
    bundle_id    TEXT NOT NULL,
    source_url   TEXT NOT NULL,
    version      TEXT NOT NULL,
    installed_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    etag         BIGINT NOT NULL DEFAULT 1,
    deleted_at   TIMESTAMPTZ,
    PRIMARY KEY (user_id, bundle_id)
);
CREATE INDEX installed_effects_by_user ON installed_effects(user_id, installed_at);
CREATE INDEX installed_effects_undeleted ON installed_effects(user_id, installed_at) WHERE deleted_at IS NULL;

CREATE TABLE sync_log (
    seq          BIGSERIAL PRIMARY KEY,
    user_id      UUID NOT NULL REFERENCES auth.users(id) ON DELETE CASCADE,
    install_id   UUID REFERENCES device_installations(id),
    entity_kind  TEXT NOT NULL,        -- 'scene', 'layout', 'favorite', etc.
    entity_id    TEXT NOT NULL,
    op           TEXT NOT NULL,        -- 'put', 'delete'
    occurred_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX sync_log_by_user_seq ON sync_log(user_id, seq);

-- Retention: rows older than the GC horizon (30 days, see Soft-delete) AND
-- whose seq is below every active sync_cursor.last_seen_seq for that user
-- are eligible for compaction. A daily background job:
--   DELETE FROM sync_log WHERE occurred_at < NOW() - INTERVAL '30 days'
--     AND seq < (SELECT MIN(last_seen_seq) FROM sync_cursor WHERE user_id = sync_log.user_id);
-- Daemons whose cursor is older than the GC horizon trigger a full resync on
-- next connect; sync_log compaction never breaks correctness.

CREATE TABLE sync_cursor (
    user_id      UUID NOT NULL REFERENCES auth.users(id) ON DELETE CASCADE,
    install_id   UUID NOT NULL REFERENCES device_installations(id) ON DELETE CASCADE,
    last_seen_seq BIGINT NOT NULL DEFAULT 0,
    last_sync_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (user_id, install_id)
);
```

### ETag semantics

`etag BIGINT` is a monotonic per-row counter incremented on every write. Clients send `If-Match: <etag>` on update. Server returns 412 Precondition Failed on mismatch. Client refetches, replays its delta, retries.

Alternative considered: hash-of-content as etag. Rejected: more expensive to compute, doesn't survive content-equivalent reformatting, hard to reason about.

### Soft-delete

`deleted_at IS NOT NULL` rows are tombstones. Clients see them in delta pulls so they can remove their local copy. Tombstones are GC'd after 30 days; clients that have been offline longer than 30 days do a full resync.

## Definition shapes

`scenes.definition`, `layouts.definition`, `profiles.snapshot` are JSONB blobs. They mirror the Rust types in `hypercolor-types` with `#[serde(default)]` on every field for forward-compat.

```jsonc
// scenes.definition
{
  "schema_version": 1,
  "render_groups": [
    { "id": "...", "name": "Desk", "device_fingerprints": ["razer:bw:..."], "effect": { ... } }
  ],
  "metadata": { "tags": ["focus", "evening"] }
}
```

### Ownership of definitions: server is opaque, daemon is canonical

The codex review caught a contradiction in the v0 draft: it said both "server stores whatever the daemon sent without parsing" and "server migrations exist." Resolved:

- **Server treats `definition` and `snapshot` as opaque jsonb.** It validates only:
  - The blob is valid JSON (Postgres `JSONB` enforces this).
  - The blob's `schema_version` is a positive integer.
  - The blob's serialized size is below 256 KB (per-row cap).
  - Any other server-side validation (e.g., that no field exceeds a length) lives in a future spec, off by default.
- **Migrations live in the daemon.** `migrate(value: serde_json::Value, from: u32, to: u32) -> Value` is a pure function in the daemon, run on read when `cached_schema_version < running_schema_version`.
- **Daemon round-trips unknown fields.** Daemon code that reads/writes definitions uses `serde_json::Value` plus typed views *constructed from* the value, never `serde_json::from_value::<Scene>(...)` followed by `to_value`. This preserves fields a newer client added that this daemon doesn't know about. The pattern:

  ```rust
  // OK: read typed for behavior, keep raw for round-trip
  let raw: serde_json::Value = fetch_definition();
  let scene: Scene = Scene::view(&raw)?;
  // ... use scene ...
  // when writing back, mutate raw directly with serde_json::Map ops
  ```

  An alternative is `#[serde(flatten)] extras: HashMap<String, Value>` on every struct, which makes round-trip preservation automatic at the cost of every struct having an `extras` field. Either works; pick per-struct based on how much round-trip you need.

- **Future server-side typed validation is opt-in.** If we ever want the server to enforce schema (e.g., to reject a 5 GB scene), it goes behind a feature flag and is documented in a follow-up spec.

This pattern matches Hypercolor's existing on-disk forward-compat in `ConfigManager`.

## Transport

### Pull (REST)

Cold start (just logged in, empty local cache):

```
GET /v1/sync/scenes
Authorization: Bearer <jwt>
→ 200 { "data": [ { id, name, definition, etag, ... }, ... ], "meta": { ... } }

GET /v1/sync/layouts ...
GET /v1/sync/favorites ...
GET /v1/sync/profiles ...
GET /v1/sync/owned-devices ...
GET /v1/sync/installed-effects ...
GET /v1/sync/prefs ...
```

Daemon stores locally, persists `sync_cursor.last_seen_seq = max(seq from sync_log for this user)`.

### Delta pull (REST)

```
GET /v1/sync/changes?since=<last_seq>
→ 200 { "data": [
    { "kind": "scene", "op": "put", "entity": { ... }, "seq": 42 },
    { "kind": "favorite", "op": "delete", "entity_id": "effect-xyz", "seq": 43 }
  ], "next_seq": 43, "has_more": false }
```

### Push (REST)

```
PUT /v1/sync/scenes/{id}
If-Match: <etag>
{ name, definition, schema_version }
→ 200 { id, etag: <new>, ... }
→ 412 { error: "stale", current_etag: <other>, current: { ... } }
```

Server increments etag, writes a `sync_log` row, broadcasts WS notification to other connected daemons of the same user.

### WebSocket push (via RFC 51 `sync.notifications` channel)

Sync push notifications ride the `sync.notifications` channel of the multiplexed daemon socket defined in RFC 51. There is no separate `/v1/sync/ws` endpoint; the v0 draft listing one was wrong and is removed.

Channel-specific frame shapes:

```jsonc
// Server → Daemon (channel: sync.notifications)
{
  "channel": "sync.notifications",
  "kind": "msg",
  "msg_id": "01J...",
  "payload": {
    "entity_kind": "scene",
    "entity_id": "...",
    "seq": 44
  }
}

// Daemon → Server (channel: sync.notifications)
{
  "channel": "sync.notifications",
  "kind": "msg",
  "msg_id": "01J...",
  "in_reply_to": "01J...",
  "payload": {
    "subscribed_since": 44
  }
}
```

WS notifications are advisory: they tell the daemon "something changed for you, pull deltas." The daemon then issues `GET /v1/sync/changes?since=<n>`. We deliberately do not push entity content over the channel to keep the source of truth single (the REST endpoints).

### Push debouncing

The daemon batches local edits per entity:

- **Scene rename + add-effect within 1.5s** → one PUT with both changes.
- **Brightness slider scrubbing** → one PUT after the user releases the slider; intermediate values are not synced.

Debounce per `(entity_kind, entity_id)` for 1500ms. Cap any single entity to one PUT every 2s (rate limit).

### Reconnect & resync

- WS dropped: exponential backoff up to 60s. Daemon falls back to 5-minute polls of `GET /v1/sync/changes?since=<n>` while WS is down.
- WS reconnect: send `{ "kind": "subscribe", "since": <last_seq> }`. If the cursor is older than the GC horizon (30 days), server returns `{ "kind": "resync_required" }`; daemon does a full pull.

## Conflict resolution

### Local upload queue model

To make auto-rebase work, the local queue must store **operational deltas, not full snapshots**. The codex review caught that storing pending PUTs as full blobs makes "rename scene" indistinguishable from "overwrite definition," which breaks rebase. Resolved with a typed mutation queue:

```rust
enum PendingMutation {
    PutScalar { entity_kind, entity_id, field_path, new_value, base_etag },
    PutCollection { entity_kind, entity_id, field_path, op: CollectionOp, base_etag },
    Create { entity_kind, entity_id, full_definition },  // first write only
    Delete { entity_kind, entity_id, base_etag },
}

enum CollectionOp { Append, Insert, Remove, Replace, Reorder }
```

Each pending mutation carries:
- `base_etag`: the etag the user's edit was relative to.
- `base_snapshot_hash`: SHA-256 over the entity at the time of the edit. Used to detect "the local cache was stale when the user made the change."
- The minimal operational delta needed to replay the edit.

Full-snapshot PUTs only happen for `Create` (no prior state to merge). Every other mutation is operational.

### Auto-rebase (the common case)

ETag mismatch on PUT triggers:

1. Client receives 412 with the server's current entity and current etag.
2. Client computes diff between `base_snapshot_hash`-time state and current server state.
3. Client replays the queued operational delta against the current server state. For most cases (different field touched, append-only collection) this succeeds unchanged.
4. Client retries the PUT with the new etag.

The "rename scene on desk-mac, add effect on laptop" case: two `PutScalar` operations on different `field_path`s. Both rebase cleanly.

### True collisions (the rare case)

If the same field collides (both machines renamed the scene, or both edited `definition`), auto-rebase fails. The daemon:

1. Keeps the server's version as canonical.
2. Saves the client's losing version as `<original_id>:conflict-<install_name>-<timestamp>` row, soft-deleted in 7 days.
3. Surfaces a toast in the daemon UI and a badge in the tray + TUI: "1 conflict. Resolve in dashboard."
4. The dashboard `/dashboard/conflicts` page shows side-by-side diff and "Use this version" buttons.

This is the Obsidian "conflict file" pattern adapted for our structured data. Never silently drop user edits.

### Conflict storage

```sql
CREATE TABLE conflicts (
    id           UUID PRIMARY KEY,
    user_id      UUID NOT NULL REFERENCES auth.users(id) ON DELETE CASCADE,
    entity_kind  TEXT NOT NULL,
    entity_id    TEXT NOT NULL,
    losing_version JSONB NOT NULL,
    losing_install UUID REFERENCES device_installations(id),
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    resolved_at  TIMESTAMPTZ,
    expires_at   TIMESTAMPTZ NOT NULL
);
```

## Schema versioning

Every blob carries `schema_version: N` at the top. **The daemon owns migrations.** `migrate(value: serde_json::Value, from: u32, to: u32) -> Value` is a pure function in the daemon, unit-tested, run on read when the cached blob's `schema_version` differs from the running daemon's. The server is opaque (per "Ownership of definitions" above): it stores `schema_version` for indexing and does not parse or migrate.

Forward-compat: older clients ignore unknown keys via `#[serde(default)]` and `#[serde(other)]` on enums.

Back-compat: newer clients fill missing fields with defaults.

Field deprecation: never repurpose a key. Add a new key, leave the old one populated for one release cycle, ignore the old one in code.

Breaking change protocol (rare): bump `schema_version`, write `migrate(value, N, N+1)`, ship to daemon and server simultaneously. Clients on `N` see new entities they cannot fully parse; they store and re-emit unknown fields untouched (round-trip preservation).

## Device library by fingerprint

`DeviceId` is locally generated per install; never sync it. Instead:

- Each install reports devices it has seen as `(fingerprint, vendor, model, last_seen_at)` to the cloud.
- Cloud stores the union under `owned_devices` keyed by fingerprint.
- On a new install, the daemon pulls `owned_devices` and recognizes hardware as it attaches: "this fingerprint is one I've seen; pre-populate name, default zones."

Fingerprint format depends on transport:

| Transport | Fingerprint shape |
|---|---|
| USB HID | `usb:vendor_id:product_id:serial` |
| USB HID without serial | `usb:vendor_id:product_id:bus_path` (less stable) |
| WLED / Hue / Nanoleaf | `network:vendor:mac` |
| WLED with no MAC visible | `network:vendor:hostname` |

Fingerprints are best-effort; we accept that some devices will appear as new on a new machine. The daemon's local `DeviceId` resolution table maps fingerprints to local DeviceIds at attach time.

## Implementation

### Crate layout

`crates/hypercolor-cloud-client/`:

- `sync/scenes.rs` — pull/push for scenes
- `sync/layouts.rs`
- `sync/favorites.rs`
- `sync/prefs.rs`
- `sync/profiles.rs`
- `sync/devices.rs`
- `sync/effects.rs`
- `sync/queue.rs` — debounced upload queue, retry with backoff
- `sync/cursor.rs` — local persisted last_seen_seq
- `sync/migrate.rs` — schema migrations (per-entity)

`~/dev/hypercolor.lighting` proprietary cloud server:

- `routes/sync.rs` — REST handlers
- `routes/sync_ws.rs` — WS push
- `domain/sync.rs` — entity types, etag logic, sync_log writes
- `migrations/` — `0001_init.sql`, etc.
- `migrations/` — `0001_init.sql`, etc.

### Local storage

Daemon already has a config directory (`~/.config/hypercolor/` on Linux, equivalents elsewhere). Sync state lands in `cloud/`:

```
~/.config/hypercolor/cloud/
  cursor.toml          # last_seen_seq, last_sync_at
  cache.sqlite         # local cache mirroring server schema (with raw jsonb preserved)
  upload_queue.sqlite  # pending PendingMutation rows (operational deltas, not blobs)
```

SQLite (via `rusqlite` 0.32) mirrors the server schema and stores raw `jsonb` (Postgres) as `TEXT` containing canonical JSON. Both the cache and the upload queue use SQLite for transactional integrity (a partial replay on crash leaves the queue consistent). The earlier "JSON files" suggestion is dropped; transactional semantics matter.

### Crate dependencies

| Crate | Version | Purpose |
|---|---|---|
| `sqlx` | 0.8 | Server-side Postgres |
| `rusqlite` or `sqlx-sqlite` | 0.30 / 0.8 | Daemon-side local cache |
| `reqwest` | 0.12 | Daemon HTTP client |
| `serde` + `serde_json` | 1.x | Entity blobs |
| `tokio-tungstenite` | 0.24 | Daemon WS client |
| `tracing` | 0.1 | Observability |

## Decisions on previously-open questions

- **Encryption at rest: plaintext jsonb in v1, honest privacy story.** Settings sync stores scene/layout/profile definitions as plaintext jsonb in Postgres. A Hypercolor Cloud operator with DB read access can see scene content. This is documented in RFC 48's threat model and surfaced in product copy: "Settings sync is encrypted in transit, not end-to-end encrypted. For end-to-end-encrypted settings sync, wait for v2." A v2 design with per-user keys derived from the user's password (Better-Auth-mediated) is parked behind that copy decision.
- **`prefs` is per-key.** More granular wins; the conflict cost is small for scalar prefs.
- **Profiles never auto-activate.** A synced profile is a saved state. User explicitly loads.
- **Tombstone GC horizon = 30 days.** Daemons offline longer trigger full resync via `last_seen_seq` mismatch.
- **Compressed deltas via `Content-Encoding: zstd`** when the response exceeds 100 KB. RFC 51's per-channel compression negotiation does not apply here (REST, not WS).

## Decision log

- **2026-05-03.** Per-row ETag with optimistic write picked over LWW (loses concurrent edits) and CRDTs (overkill for single-user multi-device).
- **2026-05-03.** Device library syncs by fingerprint, not by local DeviceId. The daemon resolves at attach time.
- **2026-05-03.** WS notifications are advisory, not authoritative. REST endpoints are the source of truth.
- **2026-05-03 (revision after codex review).** Sync push delivered via RFC 51's `sync.notifications` channel of the multiplexed daemon socket; the `/v1/sync/ws` endpoint listed in v0 is removed.
- **2026-05-03.** Local upload queue stores operational deltas (typed `PendingMutation`), not full snapshots. Auto-rebase needs the delta to replay correctly.
- **2026-05-03.** Server treats `definition` / `snapshot` blobs as opaque (size + version envelope only). Migrations live in the daemon. Daemons round-trip unknown fields via raw `serde_json::Value` views.
- **2026-05-03.** Indexing made consistent: every sync table has `(user_id, updated_at)` plus a partial `(user_id, updated_at) WHERE deleted_at IS NULL`.
- **2026-05-03.** `sync_log` retention specified: nightly compaction past 30 days AND below all active cursors.
- **2026-05-03.** Encryption at rest deferred to v2; v1 product copy honest about Hypercolor Cloud operator visibility.
