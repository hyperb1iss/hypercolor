# 61 -- Packaging and Release Hardening

> Audit findings and concrete fixes for Hypercolor's packaging, install, and CI/CD surface. Wave 1 release-blockers (§2.1-2.4) are resolved. Wave 2 hygiene items are in progress. Waves 3-4 are planned post-v0.1.0. See per-item RESOLVED/OPEN notes throughout.

**Status:** Partially resolved — Wave 1 release-blockers fixed; Wave 2 hygiene items in progress; Waves 3-4 post-v0.1.0
**Scope:** `scripts/`, `packaging/`, `.github/workflows/`, `Cargo.toml`, `rust-toolchain.toml`, `deny.toml`, `udev/`, `.gitignore`
**Author:** Nova
**Date:** 2026-04-26
**Companion to:** none — this is the first packaging spec

---

## Table of Contents

1. [Context](#1-context)
2. [Release-Blockers (must fix before v0.1.0)](#2-release-blockers)
3. [Critical for First Publish](#3-critical-for-first-publish)
4. [Important Hygiene](#4-important-hygiene)
5. [Nice-to-Have / Smaller Items](#5-nice-to-have)
6. [Out of Scope](#6-out-of-scope)
7. [Implementation Order](#7-implementation-order)
8. [Verification Plan](#8-verification-plan)
9. [Appendix: What's Solid](#9-appendix-whats-solid)

---

## 1. Context

This audit covers everything packaging-related: `justfile` recipes, `scripts/` helpers, `.github/workflows/`, `packaging/` downstream artifacts (AUR, Homebrew, desktop entry, systemd, launchd, modules-load, icons, bin shims), `Cargo.toml` workspace metadata, `deny.toml`, `rust-toolchain.toml`, and the udev rules.

**Key context:** No git tags exist yet (`git tag --list` returns empty). The CI release path (`.github/workflows/ci.yml` `build-release` matrix and `update-homebrew` job) has never actually run end-to-end. The local `dist.sh` path has run (the `dist/hypercolor-0.1.0-linux-amd64.tar.gz` artifact is present), and that path is correct. The two have drifted significantly.

The existing tarball in `dist/` is a 60 MB local build output that is currently untracked in git but visible to `git status` — easy to commit by accident.

This spec is a punch-list; once cleared, v0.1.0 can ship.

---

## 2. Release-Blockers

These would cause the v0.1.0 release pipeline to fail or produce a broken tarball.

### 2.1 CI build-release matrix references binaries that don't exist — RESOLVED

**File:** `.github/workflows/ci.yml:407`

```yaml
for bin in hypercolor hyper hypercolor-tray hypercolor-tui; do
cp "${REL}/${bin}" "${DIST}/bin/"
done
```

Three distinct bugs in this loop:

- **`hyper` is not produced by any crate.** `hypercolor-cli/Cargo.toml:12` defines `[[bin]] name = "hypercolor"`. There is no `hyper` binary; `cp` will fail under `set -eo pipefail` on the very first iteration. (Test files reference `"hyper"` only as the clap test argv0 string.)
- **`hypercolor-tui` is a library**, not a binary. `hypercolor-tui/Cargo.toml:10` sets `autobins = false` with no `[[bin]]` entry. `cargo build -p hypercolor-tui` produces no executable, so `cp ${REL}/hypercolor-tui` fails too.
- **`hypercolor-daemon` is missing entirely** from the loop. Even if the other names were fixed, the produced tarball would have no daemon binary.

The TUI is dispatched via the `packaging/bin/hypercolor-tui` shell shim that calls `exec "${SCRIPT_DIR}/hypercolor" tui "$@"`. `dist.sh:239` correctly copies this shim; CI does not.

The completions step on `ci.yml:432-434` calls `${DIST}/bin/hyper completions ...` — same wrong name. The `|| true` guards mask the failure, so the released tarball ships silently without shell completions.

**Fix applied:** `build-release` now delegates to `dist.sh`, which installs `hypercolor-daemon`, `hypercolor`, `hypercolor-tray`, and the shell shims correctly. Completion filenames updated to `_hypercolor` / `hypercolor.fish`. The library-only `-p hypercolor-tui` build step was dropped.

### 2.2 install-release.sh launchd plist runs the CLI, not the daemon — RESOLVED

**File:** `scripts/install-release.sh:449`

```xml
<key>ProgramArguments</key>
<array>
    <string>${INSTALL_DIR}/hypercolor</string>
```

This is the CLI binary, which exits immediately when invoked with no subcommand. With `KeepAlive` + `ThrottleInterval=3` the launchd agent will respawn the CLI in a tight loop on every macOS install.

The Linux/systemd path on `install-release.sh:326` correctly uses `hypercolor-daemon`. macOS does not.

**Fix applied:** `packaging/launchd/tech.hyperbliss.hypercolor.plist` now references `@BIN_DIR@/hypercolor-daemon`. `install-release.sh` uses `cp` from the tarball payload rather than an inline heredoc.

### 2.3 release.yml is missing daemon build dependencies — RESOLVED

**File:** `.github/workflows/release.yml:32-35`

```yaml
system-deps: >-
  libudev-dev libdbus-1-dev libasound2-dev
  cmake nasm pkg-config lld
  libxcb1-dev libxcb-randr0-dev libxcb-shm0-dev libxcb-xfixes0-dev
```

Missing relative to `ci.yml:21-27`'s `LINUX_DEPS`: `libpipewire-0.3-dev libpulse0 libxdo-dev libgtk-3-dev libappindicator3-dev`. Screen-capture (pipewire/portal), audio (pulse), tray (gtk3 + appindicator), and global keybind capture (xdo) all need these. The release workflow would fail to link the daemon.

**Fix applied:** `release.yml` was refactored to a tag-creation job only; the actual build runs in `ci.yml`'s `build-release` job which uses `${{ env.LINUX_DEPS }}` correctly. This item is resolved by architecture change.

### 2.4 AUR PKGBUILD pkgver is stale — RESOLVED

**File:** `packaging/aur/PKGBUILD:5`

The Cargo workspace version is `0.1.0`; `pkgver` was previously `0.2.0`, causing release source URLs to 404.

**Fix applied:** `pkgver=0.1.0` in PKGBUILD now matches `Cargo.toml version = "0.1.0"`.

---

## 3. Critical for First Publish

These don't block the build but ship a broken or unsafe end-user experience.

### 3.1 AUR PKGBUILD checksums are SKIP — OPEN (expected pre-release; populate at release time)

**File:** `packaging/aur/PKGBUILD:33-34`

```
sha256sums_x86_64=('SHA256_LINUX_AMD64')
sha256sums_aarch64=('SHA256_LINUX_AARCH64')
```

These are placeholder strings, not real hashes. That is expected behavior pre-release: the checksums cannot be computed until the release tarballs are built. They must be populated at release time before pushing to AUR. `SKIP` is not acceptable for a binary distribution — a compromised release asset would install silently.

**Fix plan:** For v0.1.0: compute sha256 sums by hand after release builds succeed and patch PKGBUILD once before AUR push. For v0.1.1+: add an `update-aur` CI job mirroring the existing `update-homebrew` job (`ci.yml:480-534`) to automate this step.

### 3.2 PKGBUILD seds the user systemd unit when the system variant exists

**File:** `packaging/aur/PKGBUILD:82-86`

The PKGBUILD runs `sed` on `lib/systemd/user/hypercolor.service` to rewrite `%h/.local/bin` → `/usr/bin` and `%h/.local/share/hypercolor` → `/usr/share/hypercolor`. But `packaging/systemd/user/hypercolor.service.system` already exists with the right paths.

The reason the sed approach works today is that the system variant isn't shipped in the dist tarball. Fix both ends:

**Fix in dist.sh:** add the `.system` variant to the Linux integration block (`scripts/dist.sh:296-300`):

```bash
cp packaging/systemd/user/hypercolor.service        "${DIST_DIR}/lib/systemd/user/"
cp packaging/systemd/user/hypercolor.service.system "${DIST_DIR}/lib/systemd/system/"
```

**Fix in CI:** mirror the same in `ci.yml:438-443`.

**Fix in PKGBUILD:** drop the sed and install `lib/systemd/system/hypercolor.service` directly into `${pkgdir}/usr/lib/systemd/system/`. Optionally also include the user unit alongside.

### 3.3 Three drifting installer scripts

`scripts/install.sh` (380 lines, local-build install), `scripts/install-release.sh` (696 lines, prebuilt tarball install), and `scripts/get-hypercolor.sh` (342 lines, hosted at `install.hypercolor.dev`) do largely overlapping work with subtle differences:

- **Different binary lists.** `install-release.sh` iterates over the binary list including `hypercolor-tui`; `install.sh` lists each `install -Dm755` call explicitly.
- **Different launchd content.** `install-release.sh` is wrong (see §2.2); `get-hypercolor.sh` doesn't generate launchd inline at all (just copies from the release payload).
- **Different completion-path cleanup.** `install-release.sh:660-663` and others reference legacy `hyper`/`_hyper`/`hyper.fish` paths from before the binary rename.
- **Two of them write systemd unit content inline** (`install-release.sh:317-342`, `get-hypercolor.sh:217-231`) instead of copying from the tarball.

The two end-user-facing installers (`install-release.sh`, `get-hypercolor.sh`) need to be unified — only one is actually published at `https://install.hypercolor.dev`.

**Fix:**

1. Pick `install-release.sh` as canonical (it's better-structured and has full uninstall coverage). Update CONTRIBUTING and any docs that reference the other.
2. Delete `get-hypercolor.sh`.
3. In the surviving installer, replace every inline-generated unit/plist with `cp` or `install -Dm644` from `${RELEASE_DIR}` — the tarball already contains correct copies.
4. Drop legacy `hyper`/`_hyper`/`hyper.fish` cleanup paths after one more release cycle.

`install.sh` (local build path) stays separate; it has different purpose and is contributor-facing only.

---

## 4. Important Hygiene

### 4.1 Shared workflows pinned to @main

**Files:** `release.yml:27`, `ci.yml:470`

```yaml
uses: hyperb1iss/shared-workflows/.github/workflows/rust-release.yml@main
uses: hyperb1iss/shared-workflows/.github/workflows/github-release.yml@main
```

A breaking change in shared-workflows silently breaks every consumer including this repo. Bliss owns the upstream so the maintenance cost is low.

**Fix:** Tag shared-workflows (e.g., `v1.0.0`) and pin both references to the tag. SHA pinning is even better but adds friction on intentional upstream changes.

### 4.2 dist/ not in .gitignore — RESOLVED

The 60 MB local tarball at `dist/hypercolor-0.1.0-linux-amd64.tar.gz` was untracked.

**Fix applied:** `/dist/` is now in `.gitignore`.

### 4.3 rust-toolchain.toml uses bare stable

**File:** `rust-toolchain.toml:2`

```toml
channel = "stable"
```

A Rust release that breaks Hypercolor will surprise both local development and CI on the same day. Given the complexity of vendor patches in `cargo-cache-build.sh:140-172` (glslopt threads_posix.h for glibc 2.39+, mozjs toolchain.configure for Xcode 26+), reproducibility matters.

**Fix:** Pin to a specific stable version that matches `Cargo.toml`'s `rust-version = "1.94"`. Bump deliberately when needed.

### 4.4 paths-filter on tag pushes

**File:** `ci.yml:36-57`

The `changes` job gates `rust-check`, `rust-test`, `rust-deny` on `outputs.rust`. `build-release` declares `needs: [rust-check, rust-test, rust-deny, sdk, ui, web-assets]`. On a tag push there's no PR base ref; `dorny/paths-filter@v3` falls back to comparing against the previous commit. If the previous commit was already a tag, or the diff is empty, the filter may evaluate to false and `build-release` blocks waiting on skipped jobs.

**Fix:** In the `changes` job, force `rust=true` when the ref is a tag:

```yaml
- id: filter
  uses: dorny/paths-filter@v3
  if: "!startsWith(github.ref, 'refs/tags/')"
  with: { ... }

- id: force_true
  if: "startsWith(github.ref, 'refs/tags/')"
  run: echo "rust=true" >> "$GITHUB_OUTPUT"

outputs:
  rust: ${{ steps.filter.outputs.rust || steps.force_true.outputs.rust }}
```

Or simpler: skip the filter entirely on tag events.

### 4.5 e2e is not a release prerequisite

**File:** `ci.yml:337`

```yaml
needs: [rust-check, rust-test, rust-deny, sdk, ui, web-assets]
```

`e2e` is excluded. This means a tag can release with the full UI/daemon Playwright suite broken.

**Decide:** add `e2e` to `needs:` if hard gating is desired, or document the choice in this spec. Recommendation: add it. The 30-minute timeout already in the e2e job caps the worst case.

### 4.6 Concurrency cancels in-flight releases

**File:** `ci.yml:15-17`

```yaml
concurrency:
  group: ${{ github.workflow }}-${{ github.ref }}
  cancel-in-progress: true
```

If a tag is pushed while the previous tag's release build is still going, the in-flight build dies — orphaned artifacts, no GitHub Release.

**Fix:** Carve out tag refs:

```yaml
cancel-in-progress: ${{ !startsWith(github.ref, 'refs/tags/') }}
```

### 4.7 Release flow undocumented

`release.yml` (workflow_dispatch) bumps version → commits → tags → pushes. The tag arriving triggers `ci.yml`'s tag-push paths (`web-assets`, `build-release`, `create-release`, `update-homebrew`). This split is fine but undocumented; when the next release happens in 6 months it will need to be re-derived.

**Fix:** Add `docs/specs/54-release-process.md` (next spec after this one) with the operator's runbook: how to dispatch `release.yml`, what triggers what, where artifacts land, how to roll back.

---

## 5. Nice-to-Have

- **No `CHANGELOG.md`.** Shared workflow generates release notes via git-iris but a tracked changelog helps AUR/Homebrew users.
- **No `dependabot.yml`.** Cargo + GH Actions auto-PRs would be cheap given the transitive dep surface.
- **`packaging/launchd/tech.hyperbliss.hypercolor.plist`** doesn't pass `--ui-dir` while the systemd unit does. Symmetry helps; use `~/Library/Application Support/hypercolor/ui` or similar.
- **`uninstall.sh` and friends still clean legacy `hyper` completion paths.** Per `b5a67ac1 fix(packaging): align cli and daemon binary names`, the rename happened. Cleanup-of-old can stay one or two more releases, then drop.

---

## 6. Out of Scope

- **Tauri desktop shell (`hypercolor-desktop`)** — explicitly excluded from default CI per `Cargo.toml:3` and `ci.yml:30`. No packaging story yet; future spec.
- **Marketing site (`site/`)** — `dist.sh:193-220` builds it opportunistically when present, but the deployment story is separate.
- **Zola docs site** — handled by `ci.yml`'s `docs:` job and `actions/deploy-pages@v4`. Working as intended.
- **Window-manager-specific desktop integration** beyond the basic `.desktop` entry.
- **Debian/Fedora/openSUSE packaging.** AUR + Homebrew + tarball cover the announced platforms.
- **Code signing / notarization for macOS builds.** Not yet a stated goal; if/when, separate spec.

---

## 7. Implementation Order

Sequenced so each change is independently mergeable and verifiable.

### Wave 1 — Release-blockers (one PR)

1. Fix `ci.yml` build-release binary list and completions (§2.1)
2. Fix `install-release.sh` launchd plist (§2.2)
3. Fix `release.yml` system-deps (§2.3)
4. Fix `PKGBUILD` pkgver to 0.1.0 (§2.4)

This PR is mechanical and self-contained. After merging, dry-run a tag push (`git tag v0.1.0-rc1 && git push origin v0.1.0-rc1`) to validate the full pipeline before the real `v0.1.0` tag.

### Wave 2 — Hygiene (separate PRs, mergeable any order)

5. `dist/` to .gitignore (§4.2) — trivial, can be in Wave 1 if convenient
6. Pin shared-workflows refs (§4.1)
7. Pin rust-toolchain (§4.3)
8. Tag-aware concurrency + paths-filter (§4.4, §4.6)
9. Add e2e to release `needs:` if desired (§4.5)
10. Bundle systemd `.system` variant in tarball + simplify PKGBUILD (§3.2)

### Wave 3 — Installer consolidation

11. Pick canonical end-user installer, delete the other, replace inline-generated content with `cp` from tarball (§3.3)

### Wave 4 — Automation backfill

12. Add `update-aur` CI job mirroring `update-homebrew` (§3.1)
13. Write `54-release-process.md` runbook (§4.7)
14. Add `dependabot.yml` and `CHANGELOG.md` (§5)

Wave 4 can land post-v0.1.0 in patch releases.

---

## 8. Verification Plan

For each wave, the validation is:

**Wave 1:**

- Push an `rc` tag to a fork or a throwaway branch with adjusted permissions.
- Confirm `build-release` matrix produces all three tarballs (linux-amd64, linux-arm64, macos-arm64).
- Extract each tarball; verify `bin/` contains `hypercolor-daemon hypercolor hypercolor-tray hypercolor-tui hypercolor-open`, that `hypercolor-tui` is the shell shim, and that `share/{bash-completion,zsh,fish}/...` are populated.
- On macOS, install the macos-arm64 tarball via `install-release.sh`, confirm `launchctl list tech.hyperbliss.hypercolor` shows the daemon running stably (no respawn loop).
- Confirm `update-homebrew` job runs without errors (existing job, but the input tarball shape is changing).

**Wave 2:**

- `cargo build --workspace --exclude hypercolor-desktop` succeeds with the pinned Rust version.
- Push two tags rapidly — confirm second one doesn't kill the first build.
- Confirm tag-push triggers all needs jobs even on commits with no Rust changes.

**Wave 3:**

- `curl -fsSL https://install.hypercolor.dev | bash` against a clean Linux VM and a clean macOS box; confirm idempotent install + uninstall cycle.

**Wave 4:**

- AUR install on a clean Arch container: `yay -S hypercolor-bin`.
- `dependabot` opens at least one PR within a week.

---

## 9. Appendix: What's Solid

To balance the picture, document what's already right so it doesn't get touched:

- **`scripts/dist.sh`** is correct and well-structured: cross-compile aware, version from cargo metadata, rich `manifest.json` with file counts, every host integration file in the right place. The CI build-release should converge toward this rather than the other way around.
- **`scripts/cargo-cache-build.sh`** does real work: clang/lld linker setup, opportunistic sccache+ccache routing, the `glslopt` and `mozjs_sys` vendor patches that keep Servo building on glibc 2.39+ and Xcode 26+. **Do not touch the patches** — they're load-bearing.
- **`Cargo.toml` profiles** are deliberate: dev/preview keep Hypercolor crates at opt-level 1-2, Servo deps at 3 even in dev, with debug-info trimmed to line tables. Release uses LTO + codegen-units=1 + strip.
- **`Cargo.toml` clippy lint config** is comprehensive — pedantic at deny, with documented allow-list for the noisy lints.
- **`udev/99-hypercolor.rules`** uses `TAG+="uaccess"` (modern systemd-logind ACL replay) with a fallback `GROUP="users", MODE="0660"`. Vendor-wide HID + USB rules so new product IDs don't need a udev update.
- **`deny.toml`** allowlist documents three unavoidable transitive Servo/Tauri vulns (`RUSTSEC-2024-0415`, `RUSTSEC-2025-0144`, `RUSTSEC-2023-0071`).
- **Issue templates, FUNDING.yml, SECURITY.md, CODE_OF_CONDUCT.md, CONTRIBUTING.md, NOTICE** all present and up to date.
- **`packaging/bin/hypercolor-open`** correctly: nudges systemd to start the daemon, polls `/health` for up to 10 s, then opens the browser. Graceful when systemd or curl is missing.

---

**Recommendation:** Land Wave 1 as a single tight PR, dry-run with an `rc` tag, then ship v0.1.0. Waves 2-4 follow in patch releases without burning anyone.
