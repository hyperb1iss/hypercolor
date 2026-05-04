# 52. RFC: Updater Client (`hypercolor-updater`)

**Status:** Draft. Resolves codex review HIGH #13 (Tauri+Velopack manifest mismatch) and decision-blockers around Velopack vs custom and headless update UX. Revised 2026-05-03 after second codex pass: separate manifest+artifact signing keys, rollback target metadata explicit, Windows helper hardening.
**Date:** 2026-05-03.
**Author:** Bliss (with Nova).
**Depends on:** [50](50-update-pipeline.md). **Companion to:** [47](47-cloud-services-overview.md).

## Summary

Hypercolor ships a single first-party Rust auto-updater crate, `hypercolor-updater`, used by every distributable surface (daemon, CLI, TUI, tray, desktop). It replaces both Tauri Updater v2 and Velopack from earlier drafts of RFC 50.

One signed manifest format (we own it). **Two distinct Ed25519 signing keys per RFC 50: `MANIFEST_SIGNING_KEY` for the manifest envelope, `ARTIFACT_SIGNING_KEY` for binary bytes.** Each rotates independently with its own `kid`-rolling window. One rollback model that actually expresses revocation and forced downgrade. Atomic platform-aware swap (`rename(2)` on Unix, signed-helper dance on Windows). Service restart through the platform supervisor (systemd / launchd / SCM / per-user). The whole thing fits in roughly 700 LOC plus a tiny signed Windows helper.

## Goals

1. **One updater for everything.** The daemon, CLI, TUI, tray, and Tauri desktop all run the same code path with platform-specific install adapters.
2. **One manifest format we control.** No converging on Tauri's or Velopack's expectations. Future-proof for our own rollback and channel features.
3. **Custom but tiny.** ~600 LOC of Rust, four trusted crates pulled in. No .NET dependency, no GitHub Releases coupling.
4. **Compile-out for OSS builds.** The `hypercolor-updater` crate is feature-gated. `cargo build` from main produces a binary with zero update code present.
5. **Honest about platform constraints.** macOS `.app` bundle replacement, Windows helper-binary self-replacement, Linux user-service polkit-free path are each spec'd, not waved at.

## Non-goals

- **Replacing package manager updates.** AUR, Homebrew, apt/dnf, MSIX, Flatpak handle their own. The updater detects "managed by package manager" and skips.
- **Running as root.** Hypercolor is a user-mode RGB daemon. The updater refuses to operate as root.
- **Forced updates.** Users always retain "stay on this version" via `daemon.toml`.
- **Delta updates.** Full artifact downloads only in v1. Binaries are ~30-50 MB; ~monthly cadence; not a bandwidth concern. Revisit if release cadence accelerates.
- **TUF.** RFC 50 keeps single Ed25519 key with rare rotation. Updater verifies that key. TUF migration is a v2 concern.

## Manifest schema

Owned by us. Served from `/v1/updates/check` (entitlement-gated edge Worker) with a fallback at `https://updates.hypercolor.lighting/manifest/{channel}.json` (R2 public, manifest still signed).

```jsonc
{
  "schema_version": 1,
  "channel": "stable",
  "current": {
    "version": "1.4.2",
    "released_at": "2026-05-15T17:00:00Z",
    "min_supported_from": "1.0.0",
    "notes_url": "https://hypercolor.lighting/release/1.4.2",
    "platforms": {
      "linux-x86_64":  { "url": "...", "size": 28473921, "blake3": "...", "minisign": "...", "kind": "tarball-zstd" },
      "linux-aarch64": { "...": "..." },
      "darwin-aarch64":{ "url": "...", "size": ..., "blake3": "...", "minisign": "...", "kind": "app-bundle-tarball-zstd" },
      "darwin-x86_64": { "...": "..." },
      "windows-x86_64":{ "url": "...", "size": ..., "blake3": "...", "minisign": "...", "kind": "msi" }
    }
  },
  "rollback_target": null,
  "revoked_versions": ["1.4.0", "1.4.1"],
  "allow_downgrade": false,
  "manifest_signature": "RWQ...",
  "issued_at": "2026-05-15T17:00:05Z",
  "manifest_kid": "minisign-2026-01"
}
```

Field semantics:

- `min_supported_from` — floor: a daemon below this version cannot accept this manifest as a single-step update; it must bootstrap-install fresh. Used to enforce migration breakpoints.
- `revoked_versions` — daemons currently running any of these versions self-quarantine (refuse render loop, surface "update required" state) until they apply `current.version` or downgrade to `rollback_target`.
- `rollback_target` — non-null forces affected daemons toward this older version on the next check.
- `allow_downgrade` — must be true for `rollback_target` < current daemon version to apply. Defense in depth against accidental downgrade pushes.
- `manifest_signature` — Ed25519 minisign over canonical-JSON bytes of the manifest minus this field.
- `manifest_kid` — key id, allowing us to roll the manifest signing key without breaking older daemons that pin the previous public key.

The daemon's compiled-in trust root is **two** public keys: the current `manifest_kid` and the previous one (rolling window of two). Rotation lands a new `manifest_kid` while still serving manifests signed by the old key for one release cycle, then drops the old.

## Rollback semantics

Three valid manifest shapes:

**Forward update** (the common case):

```jsonc
{
  "current": { "version": "1.5.0", ... },
  "rollback_target": null,
  "revoked_versions": [],
  "allow_downgrade": false
}
```

**Recall a bad release**:

```jsonc
{
  "current": { "version": "1.5.1", ... },          // the fixed version
  "rollback_target": null,
  "revoked_versions": ["1.5.0"],
  "allow_downgrade": false
}
```

Daemons on 1.5.0 self-quarantine and update to 1.5.1 immediately, ignoring rollout cohort. Daemons on 1.4.x keep working and follow the normal cohort schedule for 1.5.1.

**Forced downgrade** (no fix available, must roll back):

```jsonc
{
  "current": { "version": "1.5.0-recovery", "min_supported_from": "1.4.0", ... },
  "rollback_target": {
    "version": "1.4.7",
    "manifest_url": "https://updates.hypercolor.lighting/manifest/stable/1.4.7.json",
    "manifest_sha256": "0123abcd...",
    "manifest_kid": "manifest-2025-12",
    "artifact_kid": "artifact-2025-12",
    "platforms": {
      "linux-x86_64":   { "url": "...", "size": ..., "blake3": "...", "minisign": "..." },
      "darwin-aarch64": { "...": "..." },
      "windows-x86_64": { "...": "..." }
    }
  },
  "revoked_versions": ["1.5.0"],
  "allow_downgrade": true
}
```

The rollback target is **embedded in the active manifest with full signed metadata**, not fetched as a separate manifest. This avoids "we recall 1.5.0 but the 1.4.7 manifest URL we point at gets edited by an attacker" attacks. The daemon validates the embedded `rollback_target.platforms.*.minisign` against the same `artifact_kid` rolling window it uses for normal updates. `allow_downgrade=true` is the explicit consent flag without which the daemon refuses to install older versions.

If the embedded `manifest_kid` or `artifact_kid` is older than the daemon's rolling window of pinned keys, the daemon refuses the rollback and surfaces "manual intervention required" rather than trusting an unknown signer.

## Update lifecycle

```
periodic check (every 4h, ±30min jitter, plus immediate on launch)
   │
   ├── fetch manifest from primary endpoint with entitlement JWT
   ├── on 5xx / network error: fetch from R2 fallback URL (no entitlement)
   ├── on both fail: backoff, return cached manifest if fresh
   │
   ├── verify manifest_signature against pinned pubkeys
   ├── if our version in revoked_versions → enter quarantine state
   │
   ├── compare current.version to running version + cohort
   │   if not eligible → done
   │
   ├── stream-download artifact to staged file in destination filesystem
   │   write blake3 hash incrementally; reject on mismatch
   │
   ├── verify minisign signature on artifact
   │
   ├── set update_ready.flag with target_version, staged_path, kind
   ├── broadcast UpdateReady event on HypercolorBus
   ├── log INFO with target version
   │
   └── wait for restart trigger
        │ default: maintenance window (configurable, default 03:00-05:00 local)
        │ tray "Restart now" click → immediate
        │ CLI `hypercolor update apply` → immediate
        │ render pipeline busy at window start → defer 24h
        │
        ├── platform-specific install (atomic swap)
        ├── platform-specific service restart
        └── new process loads, removes update_ready.flag
```

Quarantine state: daemon refuses to start the render loop, exposes `/api/v1/health` returning `{"state": "quarantined", "reason": "version revoked"}`, surfaces in tray and CLI. Update check continues; once a non-revoked version is available, normal flow resumes.

## Headless UX

Three viable patterns existed; we pick **download silently, restart in maintenance window, opt-out per config**.

```toml
# daemon.toml
[auto_update]
enabled = true
channel = "stable"                  # or "beta", "nightly"
restart_window = "03:00-05:00"      # local time
notify_user = "tray"                # or "none", "email" (later)
```

Tray applet shows a coral badge when an update is staged: `Update 1.4.2 ready, will install at 3am`. Click for "Restart now" or "Skip this version."

CLI:

```
$ hypercolor update status
Channel: stable
Current: 1.4.1
Latest:  1.4.2 (released 2026-05-15)
Staged:  1.4.2 — will install in 4h 23m
Last check: 2 min ago

$ hypercolor update apply         # restart now
$ hypercolor update skip 1.4.2    # don't install this one
$ hypercolor update check         # force re-check
```

The maintenance window logic checks render-pipeline busy state: if the user has an active scene with audio reactivity, defer 24h. After three deferrals, tray escalates to a modal "An update has been waiting 3 days. Restart now?" prompt.

## Fallback when manifest endpoint is unreachable

```
primary:    https://api.hypercolor.lighting/v1/updates/check
            (entitlement-gated, edge Worker, ~50ms)

secondary:  https://updates.hypercolor.lighting/manifest/{channel}.json
            (R2 public, no entitlement check, manifest still minisigned)

cached:     ~/.local/state/hypercolor/manifest.json
            (last known good, with cached_at timestamp)
```

Backoff schedule: 1m, 5m, 30m, 4h, then 24h ceiling. After 7 days of total failure, log warning to tray ("haven't reached update server in a week"). Never block daemon functionality on update reachability.

The R2 secondary still serves a minisigned manifest, so a malicious downgrade of the entitlement service cannot serve fake updates. The entitlement gate only controls *who is eligible to receive updates*; the manifest itself is publicly verifiable.

## Entitlement grace

Daemon caches the last entitlement JWT in `~/.local/state/hypercolor/entitlement.json` with `cached_at` and `expires_at`.

| State | Behavior |
|---|---|
| Cache fresh, `update_until > now` | Normal operation, check updates |
| Cache stale, refresh succeeds | Update cache, continue normally |
| Cache stale, refresh fails, within 14d soft TTL | Use cache, continue normally, log "entitlement refresh failed, using cache" |
| Cache stale > 14d, refresh failing | Enter "frozen updates" state. Daemon keeps running fully. Skip update checks. Tray surfaces "reconnect to keep getting updates" |
| Entitlement explicitly expired (`update_until < now`) | Updates frozen at last entitled version. Tray + CLI surface "renew to resume updates" |

The OSS daemon never has this code path: `cargo build` from `main` compiles without `--features official_updates`, the entire `hypercolor-updater` crate is excluded, and the daemon has no way to call `hypercolor update *`.

## Platform install adapters

### Linux

Single binary at `~/.local/bin/hypercolor-daemon`.

1. Stream download to `~/.local/bin/.hypercolor-daemon.staging-<ulid>` (same directory = same filesystem, avoids EXDEV).
2. `fsync` and `fchmod 0755`.
3. Verify minisign over the file bytes.
4. `rename(2)` to `~/.local/bin/hypercolor-daemon`. Atomic. Running daemon's open file mapping survives.
5. Trigger service restart.

If the binary lives in a system path (`/usr/local/bin`, `/usr/bin`), the updater detects this via `argv[0]` and the path's owner UID and **refuses to self-update**: "managed by package manager, update via apt/dnf/pacman." Logs and surfaces in tray.

### macOS — single binary

Same as Linux. Stage in destination directory, fsync, rename. APFS atomic. Code signature stays valid because we replace the entire binary, not edit it in place.

### macOS — `.app` bundle (desktop shell)

`/Applications/Hypercolor.app` (or `~/Applications/Hypercolor.app`).

1. Download `.tar.zst` to `~/Library/Caches/Hypercolor/staging/`.
2. Extract to `~/Library/Caches/Hypercolor/staging/Hypercolor.app/`.
3. Verify minisign over the tarball bytes.
4. `codesign --verify --deep --strict ~/Library/Caches/Hypercolor/staging/Hypercolor.app`.
5. `rename` current `.app` to `Hypercolor.app.old-<ulid>`.
6. `rename` staged `.app` into place.
7. On `PermissionDenied` (`/Applications` requires admin), fall back to AppleScript via `osakit`: `do shell script "rm -rf ... && mv ..." with administrator privileges`. User prompted for admin password.
8. Delete `.old-*` after restart succeeds.
9. `launchctl kickstart -k gui/$UID/lighting.hypercolor.daemon`.

If `_NSGetExecutablePath` shows the app is **translocated** (running from quarantined Downloads folder), refuse to update and instruct user to drag to `/Applications` first. Tauri Updater does not handle this; we will.

### Windows — single binary

Daemon at `%LOCALAPPDATA%\Hypercolor\bin\hypercolor-daemon.exe`. Per-user, no SCM, no UAC.

1. Stream download to `%LOCALAPPDATA%\Hypercolor\bin\hypercolor-daemon.new.exe`.
2. Verify minisign over downloaded bytes.
3. Construct **signed staging manifest** (Ed25519, signed by daemon's identity key from RFC 51):

   ```jsonc
   {
     "parent_pid": 12345,
     "current_path": "%LOCALAPPDATA%\\Hypercolor\\bin\\hypercolor-daemon.exe",
     "new_path": "%LOCALAPPDATA%\\Hypercolor\\bin\\hypercolor-daemon.new.exe",
     "expected_new_sha256": "...",
     "restart_command": "start_service hypercolor-daemon",
     "issued_at": "<RFC3339 timestamp>",
     "expires_at": "<RFC3339 timestamp + 5min>",
     "nonce": "<base64 16 bytes>",
     "signature": "<Ed25519 over canonical-JSON of all above>"
   }
   ```

4. Verify hypercolor-updater-helper.exe's own Authenticode signature is intact and that its hash matches the value embedded in the daemon (the daemon ships the helper's expected SHA-256 as a compile-time constant, computed at release-build time).
5. Spawn `hypercolor-updater-helper.exe` with the signed staging manifest as its single argument (path to a temp file). Detached.
6. `exit(0)` from daemon.

The helper:

1. Reads the staging manifest, verifies signature against the same daemon identity public key that the daemon registered with the cloud.
2. Verifies `expires_at` is in the future and `nonce` has not been used (small persistent replay cache in `%LOCALAPPDATA%\Hypercolor\helper-state.db`).
3. Constrains paths: `current_path` and `new_path` must both be under `%LOCALAPPDATA%\Hypercolor\bin\`. UNC paths (`\\server\share\...`) and network paths are rejected. Symlinks resolved and re-checked.
4. Verifies SHA-256 of `new_path` matches `expected_new_sha256`.
5. Waits up to 30s for `parent_pid` to exit.
6. `MoveFileExW(current_path, current_path + ".old", MOVEFILE_REPLACE_EXISTING)`.
7. `MoveFileExW(new_path, current_path, MOVEFILE_REPLACE_EXISTING)`.
8. `restart_command` is one of a fixed enum (`start_service hypercolor-daemon`, `start_user hypercolor-daemon`, `none`); never an arbitrary command line.
9. Delete `.old` (now unlocked).
10. Helper exits.

This closes the "helper accepts arbitrary paths and restart commands" hole: the helper is signed (Authenticode), pinned by hash from the daemon, accepts only a daemon-signed manifest, restricts paths to the install root, and routes restart through a fixed command enum.

If the daemon is installed as a Windows Service, the path requires `windows-service` 0.8 to `ControlService(SERVICE_CONTROL_STOP)` first. Per-user install (Scheduled Task with LogonTrigger) is the recommended deployment, no SCM involved.

### Windows — MSI (desktop shell)

Tauri ships an MSI. The updater downloads the new MSI, verifies minisign, then runs `msiexec /i path-to-new.msi /quiet /norestart`. MSI handles its own install over the existing app. Tauri shell's update flow is *driven by* `hypercolor-updater` rather than `tauri-plugin-updater`.

## Service restart per platform

| Platform | Mechanism | Crate |
|---|---|---|
| Linux user (systemd `--user`) | `systemctl --user restart hypercolor` via `Command::new` (no polkit needed for user units) | `std::process::Command` |
| Linux user (D-Bus) | `org.freedesktop.systemd1.Manager.RestartUnit("hypercolor.service", "replace")` on session bus | `zbus` 5.x |
| Linux system service (rare) | Polkit rule shipped in package scoping `manage-units` to `hypercolor.service` for the install user | `zbus` + polkit JS rule |
| macOS LaunchAgent | `launchctl kickstart -k gui/$UID/lighting.hypercolor.daemon` | `std::process::Command` |
| Windows Service | `windows-service` 0.8 → `ControlService(STOP)` then `StartServiceW` | `windows-service` |
| Windows per-user (recommended) | Helper kills PID, spawns new exe | `std::process::Command` |

Default: install hypercolor as a **user-mode** service on every platform. No polkit, no UAC, no admin password. Aligns with the "RGB lighting doesn't need root" non-goal.

## Crate layout

```
crates/
  hypercolor-updater/
    src/
      lib.rs                    # public API: check_update, download, install, restart
      manifest.rs               # JSON schema, version compare, channel logic
      verify.rs                 # minisign-verify wrapper, kid rolling window
      download.rs               # streaming reqwest, blake3, atomic staging
      stage.rs                  # cross-platform stage-into-destination-dir
      install/
        mod.rs                  # platform dispatch
        unix.rs                 # rename swap, EXDEV-aware (Linux + macOS Mach-O)
        macos_bundle.rs         # .app replace, osakit admin fallback
        windows.rs              # spawns helper, exits
        package_managed.rs      # detects + refuses, shows guidance
      restart/
        mod.rs                  # platform dispatch
        systemd.rs              # zbus session bus
        launchd.rs              # launchctl kickstart -k
        windows_svc.rs          # windows-service crate
        windows_user.rs         # PID kill + spawn
      entitlement.rs            # cache, grace logic
      state.rs                  # quarantine, deferral, update_ready.flag
      events.rs                 # HypercolorBus integration
      error.rs

  hypercolor-updater-helper/    # tiny Windows-only binary
    src/main.rs                 # waits parent PID, rename dance, restart, exit
    Cargo.toml                  # cfg target_os = "windows" only
```

The `install` and `restart` modules are independent axes: a daemon on Linux composes `install::unix` + `restart::systemd`; a CLI on Linux uses `install::unix` + no restart. The four target binaries pick what they need.

## Crate dependencies

| Crate | Version | Purpose |
|---|---|---|
| `minisign-verify` | 0.2 | Manifest + artifact signature verification, zero deps |
| `self-replace` | 1.5 | Used on Windows for the helper-spawn pattern; we hand-roll Unix swap |
| `reqwest` | 0.12 | HTTP, streaming download, rustls |
| `blake3` | 1.5 | Streaming artifact hash |
| `zstd` | 0.13 | Tarball decompression on macOS/Linux |
| `tar` | 0.4 | Tarball extraction |
| `tempfile` | 3.10 | Atomic staging via `NamedTempFile::persist_into` |
| `serde` + `serde_json` | 1.x | Manifest |
| `zbus` | 5.x | systemd D-Bus on Linux |
| `windows-service` | 0.8 | Windows SCM (only if SCM mode chosen) |
| `osakit` | 0.x | macOS AppleScript admin elevation fallback |
| `notify-rust` | 4.x | Optional desktop notifications |
| `jsonwebtoken` | 9.x | Entitlement JWT verification |
| `keyring` | 3.x | Cached entitlement |
| `tracing` | 0.1 | Logging |

No .NET. No Velopack. No tauri-plugin-updater.

## Feature flags

`hypercolor-updater` gates the entire crate behind a `cargo` feature in the consuming binaries:

```toml
# crates/hypercolor-daemon/Cargo.toml
[features]
default = []
official-cloud = ["dep:hypercolor-cloud-client"]
official-updates = ["dep:hypercolor-updater"]
```

Official builds enable both. Community builds enable neither, and the binary genuinely contains zero update or cloud client code.

## CI release workflow

```yaml
on: push tags v*
jobs:
  build-matrix:
    strategy: { matrix: { target: [
      x86_64-unknown-linux-gnu, aarch64-unknown-linux-gnu,
      x86_64-apple-darwin, aarch64-apple-darwin,
      x86_64-pc-windows-msvc
    ]}}
    steps:
      - cargo build --release --locked --features official-cloud,official-updates --target=$TARGET
      - sign + notarize per OS  (codesign + notarytool, signtool via Azure Artifact Signing, no-op for Linux)
      - tar.zst the artifact (or .msi for Windows desktop, .app.tar.zst for macOS desktop)
      - minisign sign the artifact bytes
      - upload to R2 at releases/<version>/<artifact>

  publish-manifest:
    needs: [build-matrix]
    steps:
      - construct manifest from build outputs
      - minisign the canonical-JSON of the manifest
      - upload manifest to R2 at manifest/<channel>.json
      - upload to Workers KV (drives /v1/updates/check edge endpoint)
      - POST /v1/admin/releases on the proprietary cloud server (records the release for cohort logic)

  start-rollout:
    needs: [publish-manifest]
    steps:
      - cohort_cap = 1   (1% initial)
      - manual gh workflow run rollout.yml -f version=1.5.0 -f cohort=10
        expands to 10/50/100 over time
```

## Open questions

None blocking implementation. Future work:

1. **Delta updates.** If artifact size grows or release cadence accelerates, swap full downloads for binary diffs. `bsdiff` or `xdelta` plus a "latest 3 versions" matrix.
2. **Background pre-download on metered connections.** Skip auto-download when `daemon.toml` says `metered = true`; only fetch on user request.
3. **Update telemetry.** Anonymous "update applied successfully" pings to detect rollout regressions, opt-in.

## Decisions

- **2026-05-03.** Custom Rust updater picked over Tauri Updater + Velopack split. Resolves codex HIGH on dual-manifest mismatch. ~600 LOC vs two third-party libraries with conflicting expectations.
- **2026-05-03.** Two distinct Ed25519 keys: `MANIFEST_SIGNING_KEY` for the manifest envelope, `ARTIFACT_SIGNING_KEY` for binary bytes (minisign format). Each rotates independently with its own rolling window of two pinned pubkeys.
- **2026-05-03.** Rollback model uses `revoked_versions` + `rollback_target` + `allow_downgrade`. Resolves codex HIGH on `min_safe_version` semantics.
- **2026-05-03.** Headless update UX: download silent, restart in maintenance window (default 03:00-05:00 local), defer if render pipeline busy, escalate after 3 deferrals. Resolves decision-blocker.
- **2026-05-03.** Manifest fallback chain: entitlement-gated primary → R2 public secondary → cached. Backoff to 24h ceiling, 7-day warn. Resolves decision-blocker.
- **2026-05-03.** Entitlement grace: 14-day soft TTL on cache, then "frozen updates" state without breaking app function. JetBrains-style perpetual-fallback inspired.
- **2026-05-03.** Hypercolor installs as a user-mode service everywhere by default. No polkit, no UAC, no admin password. Aligns with "no root for RGB" principle.
- **2026-05-03.** Tauri desktop shell consumes `hypercolor-updater` directly via a Tauri command rather than running `tauri-plugin-updater` in parallel. One updater, one manifest, no divergence risk.
