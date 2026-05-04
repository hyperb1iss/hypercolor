# 50. RFC: Update Pipeline & Signed Builds

**Status:** Draft. Revised 2026-05-03 after codex review and RFC 52 split.
**Date:** 2026-05-03.
**Author:** Bliss (with Nova).
**Depends on:** [25](25-distribution-and-applet.md), [46](46-cross-platform-packaging.md), [47](47-cloud-services-overview.md). **Companion:** [52](52-updater-client.md).

## Summary

Logged-in users get signed official builds with automatic updates. The OSS build path (clone, `cargo build`, run) stays free, unsigned, and self-update-free forever. The cloud issues entitlement JWTs (per RFC 47's canonical schema) the daemon presents on `/v1/updates/check`; the update server returns a signed manifest pointing at a CDN.

This RFC owns the **server side** of the pipeline: signing, channels, rollouts, CDN, manifest publishing, admin endpoints, CI workflow. RFC 52 owns the **client side** (`hypercolor-updater` crate), including platform install adapters, service restart, fallback behavior, and headless UX. The previous draft proposed Tauri Updater + Velopack on the client; that split is replaced by a single custom Rust updater (RFC 52).

## Goals

1. **Logged-in users on official builds get auto-updates** within hours of release.
2. **Signing and notarization on macOS / Windows / Linux** without breaking existing AUR / Homebrew / curl-installer paths.
3. **Phased rollout with rollback.** Push to 10% first, watch metrics, expand or recall.
4. **Cheap.** Sub-$500/year all-in (signing certs + CDN).
5. **Three distinct signing keys, none of them shared.** `ARTIFACT_SIGNING_KEY` (binaries), `MANIFEST_SIGNING_KEY` (manifest envelopes including rollout policy), `ENTITLEMENT_SIGNING_KEY` (cloud-issued entitlement JWTs). All Ed25519, rotated independently. Compromise of any one does not collapse the others.
6. **Effects are signed separately.** User-generated content has a different threat model and key hierarchy.

## Non-goals

- **Encrypting binaries.** DRM theater. We sign and verify integrity; we do not gate decryption on entitlements.
- **TUF (The Update Framework).** Right answer eventually, overkill for v1. Single key + rare rotation gets us 95% of the security benefit at 5% of the operational complexity. Revisit when we have someone whose job is supply chain.
- **In-place auto-update for distro packages.** apt, AUR, Homebrew, Flatpak handle their own. We auto-update only the binaries we ship directly.
- **Reproducible builds.** Nice-to-have, not gating. SOURCE_DATE_EPOCH and `cargo --locked` give us 80% reproducibility for free; stop there.
- **Forced updates.** Users always retain "stay on this version" as an option.

## Architecture

```
GitHub Actions release workflow
    ├── cargo build --release --target=*
    ├── codesign + notarize (macOS)
    ├── signtool via Azure Artifact Signing (Windows)
    ├── minisign (Linux tarballs, AppImage)
    ├── upload artifacts to Cloudflare R2
    ├── generate signed manifest, upload to R2
    └── publish manifest pointer to /v1/updates/manifest

Daemon / TUI / CLI / Desktop                  Cloud
    │                                            │
    │ entitlement JWT (Ed25519, exp ~1h, cached) │
    │                                            │
    ├──── GET /v1/updates/check?...  ───────────►│
    │                                            │ (Cloudflare Worker)
    │                                            │ verify entitlement
    │                                            │ check cohort_cap
    │                                            │ return manifest or 204
    │◄────  signed manifest or 204  ─────────────┤
    │                                            │
    ├──── GET cdn.hypercolor.lighting/...  ─────►│ (R2)
    │◄────  artifact bytes  ─────────────────────┤
    │                                            │
    ├── verify minisign sig vs pinned pubkey
    ├── verify entitlement covers this channel
    └── swap binary, prompt restart
```

## Client per surface

Every binary uses the same `hypercolor-updater` crate (RFC 52). One signed manifest schema, two distinct Ed25519 keys (manifest envelope + binary artifacts), one rollback model. Earlier draft's Tauri-Updater-plus-Velopack split is replaced.

| Surface | Updater | Notes |
|---|---|---|
| `hypercolor-desktop` (Tauri shell) | `hypercolor-updater` via a Tauri command | Tauri Updater plugin not used; one updater everywhere |
| `hypercolor-daemon` (headless) | `hypercolor-updater` | systemd / launchd / Windows per-user service restart |
| `hypercolor-cli` | `hypercolor-updater` | No restart hook; user-invoked |
| `hypercolor-tui` | `hypercolor-updater` | Same |
| `hypercolor-tray` | `hypercolor-updater` (shared with desktop bundle) | Surfaces "update ready" badge |
| Effect bundles | Per-author minisign + marketplace index sig | Different threat model, see below |

RFC 52 owns the implementation. This RFC continues to define the manifest the updater consumes, the signing pipeline that produces it, and the rollout/recall control loop on the server side.

## Signing per OS

### macOS

- **Apple Developer Program: $99/year.**
- Get a Developer ID Application certificate.
- Sign with `codesign --options runtime --timestamp <binary>`.
- Notarize with `xcrun notarytool submit --wait`.
- Store creds via `notarytool store-credentials` using an App Store Connect API key (not app-specific password; keys don't expire and rotate cleanly in CI).
- GitHub Actions step: `lando/notarize-action` is the maintained option in 2026.

Notarization typically resolves in 2-15 minutes. 2026 saw sporadic multi-hour Apple delays, so the CI workflow retries with exponential backoff up to 4 hours before failing the release.

### Windows

- **Azure Artifact Signing (formerly Trusted Signing). $9.99/mo basic tier covering 5,000 signatures.**
- Open to self-employed individuals (US/CA/EU/UK) as of 2026.
- Replaces EV certs entirely. Confers instant SmartScreen reputation. Identity validation takes a few days the first time.
- Sign with `signtool` against the Azure-hosted certificate. GitHub Actions: `azure/trusted-signing-action`.

Do not buy a Sectigo / DigiCert EV cert in 2026. Microsoft's own offering replaced that market.

### Linux

- **minisign per-binary signature.** `ARTIFACT_SIGNING_KEY` (Ed25519), distinct from `MANIFEST_SIGNING_KEY` (see "Manifest format" above and "Signing keys" below).
- AppImage: embed zsync2 + minisign sigfile. Standard pattern.
- Tarball: detached `.minisig` next to it.
- deb / rpm / Flatpak: out of scope (handled by package manager). The AUR package already ships SHA256 sums; add a detached minisign sig and a check-script.

No PKI involvement. No CA dependency. minisign is the right shape for Linux desktop binaries.

### CI secret matrix

| Secret | Where | Rotation cadence |
|---|---|---|
| `APPLE_API_KEY` (.p8, base64) | GitHub Actions | When the human leaves; otherwise never |
| `APPLE_API_KEY_ID` | GitHub Actions | With the key |
| `APPLE_ISSUER_ID` | GitHub Actions | Account-level, never |
| `APPLE_DEVELOPER_ID_CERT` (.p12, base64) + password | GitHub Actions | Annual (cert expires) |
| `AZURE_TENANT_ID`, `AZURE_CLIENT_ID` | GitHub Actions OIDC federated identity | OIDC, no long-lived secret |
| `ARTIFACT_SIGNING_KEY` (Ed25519, minisign format) + password | GitHub Actions | Every 2 years |
| `ENTITLEMENT_SIGNING_KEY` (Ed25519) | Cloud server config | Every year |
| `MANIFEST_SIGNING_KEY` (Ed25519) | GitHub Actions | Every 2 years (separate from artifact key) |
| `R2_ACCESS_KEY` + `R2_SECRET` | GitHub Actions | OIDC-federate when Cloudflare supports it; otherwise quarterly rotation |

Use OIDC federated identity for AWS / Azure where possible. Fewer long-lived secrets in GitHub.

## Entitlement model

The entitlement JWT schema is **canonical in RFC 47**; this RFC does not redefine it. The updater specifically uses:

- `aud` includes `"hypercolor-updater"`.
- `features` includes `"hc.signed_builds"` to gate update access.
- `channels` lists allowed channels (e.g., `["stable", "beta"]`).
- `update_until` (long-lived, 1 year typical) governs the offline grace window — `exp` (1 hour) ensures fresh online checks, but a daemon offline for up to 14 days may still apply staged updates as long as `update_until > now`. RFC 52 owns the cache + grace logic.

**Verification crate:** `jsonwebtoken` 9.x with EdDSA, plus a small custom JWKS-style cache for the entitlement signing key. `biscuit-auth` is rejected for v1; revisit only if multi-service capability delegation becomes a real need.

## Manifest format

The canonical manifest schema is defined in **RFC 52** and supports forward updates, recalls (`revoked_versions`), and forced downgrades (`rollback_target` + `allow_downgrade`). Summary of the rollout-control fields owned by the server-side loop:

```jsonc
{
  "schema_version": 1,
  "channel": "stable",
  "current": {
    "version": "1.4.2",
    "released_at": "2026-05-15T17:00:00Z",
    "min_supported_from": "1.0.0",
    "platforms": { "linux-x86_64": { "url": "...", "minisign": "...", ... }, ... }
  },
  "rollback_target": null,
  "revoked_versions": ["1.4.0", "1.4.1"],
  "allow_downgrade": false,
  "manifest_signature": "RWQ...",
  "manifest_kid": "minisign-2026-01"
}
```

Two distinct trust roots:

- Per-platform `minisign` covers the **artifact bytes**. Signed in CI by `ARTIFACT_SIGNING_KEY` (Ed25519). Carries its own `artifact_kid`.
- `manifest_signature` covers the **manifest envelope** (every field except itself, canonical-JSON). Signed at publish time by `MANIFEST_SIGNING_KEY` (Ed25519, distinct from artifact key). Carries `manifest_kid`.

Two distinct keys, not "two signatures with one key." A compromise of `MANIFEST_SIGNING_KEY` cannot MITM artifact bytes; a compromise of `ARTIFACT_SIGNING_KEY` cannot serve a fake manifest with rollout policy changes. Daemons pin **both** the current and previous keys for **each** key (rolling window of two for each via `artifact_kid` and `manifest_kid`).

## Channels & rollout

Three channels:

- **stable.** Default for everyone. Releases ~monthly.
- **beta.** 1-2 weeks ahead of stable. Opt-in via dashboard.
- **nightly.** Rolling, opt-in for tinkerers. Sometimes broken; that's the deal.

Channel selection is a daemon config knob and an entitlement scope. A user on the free tier defaults to `stable`. Beta and nightly require entitlement `channels: [..., "beta"]` or `[..., "nightly"]` (free, just opt-in flag in dashboard).

### Phased rollout

The manifest endpoint is a Cloudflare Worker. Per request, it computes:

```
cohort = stable_hash(daemon_id, version) % 100
```

If `cohort >= cohort_cap` (current rollout percentage), Worker returns 204 No Content (no update available *for you yet*).

Standard rollout schedule: 1% → 5% → 10% → 25% → 50% → 100%, with at least 12 hours between expansions and a 24-hour bake at each stage above 10%. Manual expansions; automation comes later.

**Pause:** freeze `cohort_cap`. Daemons in already-rolled-out cohorts keep the new version; everyone else stays on previous.

**Recall a bad release** (preferred path when a fix exists):

- Publish `current.version = 1.5.1` (the fixed version).
- Add `1.5.0` to `revoked_versions`.
- Daemons on 1.5.0 self-quarantine (RFC 52) and update to 1.5.1 immediately, ignoring cohort.
- Daemons on 1.4.x follow the normal cohort schedule for 1.5.1.

**Forced downgrade** (when no fix is ready and we must roll back):

- Publish `current.version = 1.5.0-recovery` with `rollback_target` carrying full **embedded signed metadata** (per RFC 52: `manifest_url`, `manifest_sha256`, `manifest_kid`, `artifact_kid`, per-platform artifact URLs and minisigns).
- Add `1.5.0` to `revoked_versions`. Set `allow_downgrade: true`.
- Daemons on 1.5.0 install 1.4.7 from the embedded metadata, not by fetching the older manifest separately. This closes "attacker tampered with the older manifest URL between recall and download" attacks.
- The earlier draft's `min_safe_version`-as-rollback-target was wrong. `min_safe_version` is a **floor** (refuse to update from anything older than this), not a target.

Per-version metrics:

- daemon_install rate
- daemon_crash rate (delta vs prior version)
- daemon_panic rate (specific Rust panic counts via `panic-handler`)
- error rate on cloud API from this version

If any metric regresses >2x at the 10% bake, auto-pause the rollout and ping on-call.

## Hosting & CDN

**Cloudflare R2** for artifacts and manifest cache. Zero egress, $0.015/GB storage, S3-compatible API. At small scale (5000 users, 100MB binary, 4 updates/year, ~2TB egress) you'd pay $0 egress on R2 vs ~$180 on raw S3.

**Cloudflare Workers** for `/v1/updates/check`. Edge-cached, $5/mo for 10M requests, JWT verification at the edge. Worker code path:

```
1. parse Authorization header → entitlement JWT
2. verify Ed25519 sig, exp, channel scope
3. compute cohort
4. look up latest manifest for (channel, platform, cohort_cap >= cohort)
5. return manifest or 204
```

Worker has a 50ms CPU budget, plenty for JWT verify + KV lookup. Manifest cache lives in Workers KV; release workflow updates KV after R2 upload completes.

## Effect signing (separate infrastructure)

User-generated marketplace effects use a **per-author minisign key + marketplace index signature**. This is intentionally different from the binary signing infrastructure.

```
Effect bundle:
  bundle.tar.gz
    manifest.json           # name, version, author, permissions, sha256s
    effect.html             # the actual effect
    assets/

Author signature:           # per-author minisign key, kept by author
  bundle.tar.gz.minisig     # signs bundle.tar.gz

Marketplace index:          # daily-rebuilt index of all approved bundles
  effects/index.json        # { author_pubkeys, bundle_metadata, ... }
  effects/index.json.sig    # marketplace's Ed25519 sig, MANIFEST_SIGNING_KEY-adjacent

Daemon verifies, in order:
  1. marketplace index sig (proves "we approved this bundle exists")
  2. author signature against bundle.tar.gz (proves "the author published this exact bytes")
  3. permissions in manifest are within author's tier
```

Two-layer trust:

- **Marketplace says** "this author is registered and not revoked, this bundle is in the catalog."
- **Author says** "I made this bundle, here are these specific bytes."

This lets us revoke a bad author without invalidating the whole catalog and lets community publishers sign without going through our CI. Same minisign tooling, totally different key hierarchy.

Authors register their public key during marketplace signup. Revocation is a flip in the marketplace index; revoked author's existing bundles stop verifying on next index pull.

## API surface

Daemon-facing:

```
GET /v1/updates/check
Headers:
  Authorization: Bearer <entitlement-jwt>
  User-Agent: hypercolor/0.5.2 (macos; arm64; channel=stable)
Query:
  current=0.5.2
  channel=stable
  os=macos
  arch=aarch64
  daemon_id=<uuid>

→ 204 No Content (no update / not in cohort)
→ 200 application/json: <manifest>
```

Daemon polls every 6 hours with ±30 minute jitter. On launch, polls immediately. Manual "check for updates now" in tray UI bypasses cache.

Admin-facing (proprietary cloud server in `~/dev/hypercolor.lighting`, gated by admin role):

```
POST /v1/admin/releases             { version, channel, platform, url, sha256, sig }
PATCH /v1/admin/releases/{v}        { cohort_cap }   # rollout control
DELETE /v1/admin/releases/{v}       { reason }       # recall: adds version to revoked_versions
GET /v1/admin/releases/{v}/metrics  # crash/error rates
```

## Implementation

### Crates

Client implementation is owned by **RFC 52** (`crates/hypercolor-updater/`). Server-side pieces of this RFC live in `~/dev/hypercolor.lighting`:

- `src/server/domain/manifest.rs` — manifest construction and signing.
- `src/server/domain/cohort.rs` — stable hash, cohort_cap arithmetic.
- `src/server/routes/admin/releases.rs` — admin endpoints.
- `src/server/routes/me/entitlements.rs` — JWT minting.
- `workers/updates-check/` — Cloudflare Worker for the public `/v1/updates/check` endpoint.

### Server-side crate dependencies

| Crate | Version | Purpose |
|---|---|---|
| `jsonwebtoken` | 9.x | Entitlement JWT minting (EdDSA) |
| `ed25519-dalek` | 2.x | Ed25519 sign for manifest |
| `minisign` | 0.7 | minisign signing of artifacts (CI side) |
| `serde` + `serde_json` | 1.x | Manifest construction |
| `sqlx` | 0.8 | Release ledger + cohort state |

### Cloud server

`~/dev/hypercolor.lighting` adds:

- `routes/admin/releases.rs` — admin endpoints
- `routes/me/entitlements.rs` — issues entitlement JWTs (canonical schema in RFC 47: short `exp`, long `update_until`)
- `domain/manifest.rs` — manifest types, signing
- `domain/cohort.rs` — stable hash, cohort_cap arithmetic

### Cloudflare Worker

`workers/updates-check/` lives in `~/dev/hypercolor.lighting` (Wrangler / wrangler.toml). TypeScript, ~150 LOC, calls Workers KV for manifest lookup, returns manifest or 204. Deployed via GitHub Actions on cloud service release.

### CI release workflow

`.github/workflows/release.yml`:

```
on: push tags v*
jobs:
  build-matrix:
    strategy: { matrix: { os: [macos-14, ubuntu-22.04, windows-latest] } }
    steps:
      - cargo build --release --locked
      - sign + notarize (per OS)
      - upload artifact to R2
      - emit { url, sha256, size, sig } as workflow output

  publish-manifest:
    needs: [build-matrix]
    steps:
      - construct manifest from build outputs
      - sign manifest with MANIFEST_SIGNING_KEY
      - upload manifest to R2 + Workers KV
      - POST /v1/admin/releases on the cloud server
      - rollout starts at cohort_cap=1
```

Manual GitHub Actions trigger for cohort_cap expansions: `gh workflow run rollout.yml -f version=0.6.0 -f cohort=10`.

## Cost projection

| Item | Cost | Notes |
|---|---|---|
| Apple Developer Program | $99/yr | Mandatory for macOS notarization |
| Azure Artifact Signing | $9.99/mo = $120/yr | Phase 4+; replaces EV cert market |
| Cloudflare Workers Paid | $5/mo = $60/yr | Phase 4+ for sub-50ms manifest checks |
| Cloudflare R2 storage | ~$0.50/mo at low scale | ~30 GB of historical releases |
| Cloudflare R2 egress | $0 | Zero egress is the point |
| **Year 1 update-pipeline total** | **~$280/yr** | |

Note: this is the **signing + CDN** cost specifically. Compute and DB costs are in RFC 47's baseline budget. Update-check traffic itself adds negligible egress to Workers (manifests are <2 KB).

## Decisions on previously-open questions

- **Updater split (Velopack vs Tauri vs custom).** Resolved: drop both, custom Rust updater per RFC 52. One manifest schema, two distinct signing keys (manifest and artifact), no .NET dependency.
- **Linux package manager updates.** Daemon detects "managed by package manager" via env vars (`FLATPAK_ID`, `SNAP`, `BREW_PREFIX`) and `argv[0]` ownership; refuses self-update and surfaces "update via your package manager."
- **Headless daemon UX.** Download silent, restart in maintenance window (default 03:00-05:00 local, configurable), defer if render pipeline busy, escalate after 3 deferrals. Owned by RFC 52.
- **Fallback when manifest endpoint unreachable.** Two-tier: primary entitlement-gated `/v1/updates/check`, secondary unauthenticated R2 mirror at `updates.hypercolor.lighting/manifest/{channel}.json` (manifest still minisigned). Both have to fail before we use cached. Owned by RFC 52.
- **Entitlement grace.** 14-day soft TTL on cached entitlement before "frozen updates" state; daemon never breaks. Owned by RFC 52.
- **Code signing continuity.** Runbook lives at `docs/runbooks/signing.md` (to be written) and includes Apple Developer renewal, Azure Artifact Signing rotation, and minisign key handoff.

## Decision log

- **2026-05-03.** Azure Artifact Signing picked over EV certs. Replaces the EV market in 2026, $9.99/mo for individuals, instant SmartScreen reputation.
- **2026-05-03.** Cloudflare Workers + R2 picked for manifest hosting. Workers gets us edge-cached entitlement checks; R2 has zero egress. Total ~$60-120/yr.
- **2026-05-03.** Effect signing kept on a separate key hierarchy (per-author minisign + marketplace index). Different threat model; conflating with binary signing is a footgun.
- **2026-05-03.** TUF deferred to v2. Two-key (manifest + artifact) scheme with rare rotation is the right cost-benefit point for now.
- **2026-05-03 (revision after codex review).** Tauri Updater + Velopack split dropped. Replaced by single custom `hypercolor-updater` crate (RFC 52). One manifest schema; manifest and artifact each get their own signing key.
- **2026-05-03.** Rollback model fixed: `revoked_versions` (force quarantine + update), `rollback_target` (forced downgrade), `allow_downgrade` (explicit consent). Earlier draft's `min_safe_version`-as-target was wrong; `min_safe_version` remains as a **floor** (refuse update from anything older).
- **2026-05-03.** Entitlement JWT schema canonicalized in RFC 47; this RFC references it rather than redefining.
