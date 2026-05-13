# Hypercolor Launch Hardening Plan

**Status:** Local hardening complete; explicit e2e/RC approval pending
**Created:** 2026-05-13
**Scope:** public repo launch, v0.1.0 release readiness, announcement readiness
**Source:** multi-agent launch readiness audit

## Verdict

The local launch-hardening work is complete. Hypercolor now has green local
verification for the security, packaging, SDK, UI, Python, docs, compatibility,
and supply-chain gates covered by this plan.

Two launch approval gates remain before public announcement:

- `just e2e`, because it starts the daemon/browser stack.
- The RC workflow/tag rehearsal, because release tags and workflow dispatches
  require an explicit go-ahead.

This plan turns the original blockers into executable waves. Each task has a
concrete verification gate. Work should land as small, goal-aligned
Conventional Commit commits with bodies.

## Success Criteria

Hypercolor is launch-ready when all of the following are true:

- Secure defaults prevent unauthenticated non-loopback REST, WebSocket, and MCP control.
- Fresh-clone install instructions work on a clean supported Linux machine.
- Release workflow can cut an RC without manual patching or hidden dependency on PyPI/npm state.
- Public docs match the current codebase and do not advertise unavailable packages.
- Public API examples use the `{ data, meta }` envelope correctly.
- Platform support is described honestly: Linux-first, macOS partial, Windows experimental unless CI proves otherwise.
- Unsafe-code policy is accurate and documents audited platform interop exceptions.
- Generated compatibility docs are current and `zola check` is green.
- The repo has no accidental large untracked launch artifacts.
- Local gates pass with receipts:
  - `just verify`
  - `just compat-check`
  - `just sdk-lint`
  - `just sdk-check`
  - `just sdk-build`
  - `just ui-test`
  - `just ui-build`
  - `just python-verify`
  - `just docs-build`
  - `cd docs && zola check`
  - `just deny`
  - `just e2e-build`
- Explicit approval gates pass:
  - `just e2e`
  - release RC workflow dry run

## Current Green Receipts

These passed during launch hardening:

- `just verify` -> `All checks passed`
- `just compat-check` -> compatibility matrix current, `31 vendors`, `410 devices`, `175 supported`
- `just sdk-lint` -> Biome checked 129 files, no fixes needed
- `just sdk-check` -> SDK and create-effect typechecks passed
- `just sdk-build` -> SDK packages built
- `just ui-test` -> UI Rust tests passed
- `just ui-build` -> Trunk release build succeeded
- `just python-verify` -> Ruff, format, ty, protocol check, and `49 passed`
- `just docs-build` -> Zola built 20 pages and 6 sections
- `cd docs && zola check` -> `20 pages`, `6 sections`, done
- `just deny` -> advisories, bans, licenses, and sources ok
- `just e2e-build` -> daemon/CLI build, effects build, and production UI build succeeded
- `git status --short --untracked-files=all` -> clean

Known skipped gates:

- `just e2e` has not been run because it starts the daemon/browser stack.
- RC workflow/tag rehearsal has not been run because release orchestration needs
  explicit approval.

## Wave 0: Branch Hygiene And Artifact Triage

### Task 0.1: Create A Dedicated Launch Branch

**Files:** none  
**Depends on:** none  
**Parallel:** no

Implementation:

- Create or switch to a dedicated branch such as `launch/v0.1-hardening`.
- Do not build on the stale `task/1d0d...` branch whose upstream is gone.
- Confirm no unrelated staged work exists.

Verify:

- `git status --short --branch`
- Branch name is launch-scoped.
- No staged unrelated files.

### Task 0.2: Triage Untracked Artifacts

**Files:** `dist/`, `effects/screenshots/`, `docs/review/`, `docs/specs/53-packaging-release-hardening.md`, `t`, `.codex`  
**Depends on:** Task 0.1  
**Parallel:** yes, read-only classification can run alongside planning updates

Implementation:

- Classify each untracked artifact as keep, track, archive, regenerate, or delete.
- Decide whether `docs/review/track-*.md` should become tracked audit records.
- Decide whether `docs/specs/53-packaging-release-hardening.md` should be updated and tracked, or replaced by this plan.
- Keep generated release tarballs and built HTML effects out of git unless explicitly intended.
- Treat `effects/screenshots/curated/` as a product asset decision, not random output.

Verify:

- `git status --short --untracked-files=all`
- `du -sh dist effects t docs/review docs/specs/53-packaging-release-hardening.md`
- No large binary artifacts are accidentally staged.

## Wave 1: Security Blockers

### Task 1.1: Refuse Non-Loopback Startup Without Control Auth

**Files:** `crates/hypercolor-daemon/src/daemon.rs`, `crates/hypercolor-daemon/src/api/security.rs`, daemon startup tests  
**Depends on:** Wave 0  
**Parallel:** no

Implementation:

- Change non-loopback bind behavior from warning-only to fail-closed when no control API key exists.
- Cover both explicit non-loopback bind and `network.remote_access = true`.
- Keep localhost defaults frictionless.
- Make the error message actionable and point to `HYPERCOLOR_API_KEY`.

Verify:

- A daemon test proves `--listen-all` without `HYPERCOLOR_API_KEY` errors before serving.
- A daemon test proves localhost without key still starts.
- A daemon test proves non-loopback with key starts.
- `cargo test -p hypercolor-daemon api::security`
- `cargo test -p hypercolor-daemon --test startup_tests`

### Task 1.2: Make Auth And CORS Config Honest

**Files:** `crates/hypercolor-types/src/config.rs`, `crates/hypercolor-daemon/src/api/mod.rs`, relevant docs/tests  
**Depends on:** Task 1.1  
**Parallel:** no

Implementation:

- Remove the stale web auth config field from public config.
- Wire `web.cors_origins` so configured origins are honored only when API key auth is active.
- Add tests for loopback defaults, configured origins, and remote access.
- Update docs to match actual behavior.

Verify:

- `cargo test -p hypercolor-types --test config_tests`
- `cargo test -p hypercolor-daemon --test security_api_tests`
- `rg -n "cors_origins|auth config field" docs crates` shows only truthful references.

### Task 1.3: Harden Driver Credential Storage

**Files:** `crates/hypercolor-driver-api/src/net/credentials.rs`, credential store tests  
**Depends on:** Wave 0  
**Parallel:** yes, can run alongside Task 1.2 if file ownership is separate

Implementation:

- Enforce `0600` permissions for `.credential_seed` and `credentials.json.enc` on Unix.
- Ensure temp files used during atomic writes are also created with restrictive permissions.
- Add migration behavior for existing too-open files.
- Decide whether OS keyring migration is required before v0.1.0 or can be a follow-up.

Verify:

- Unix tests assert seed/store permissions.
- Existing credential tests still pass.
- `cargo test -p hypercolor-driver-api --test credential_store_tests`

### Task 1.4: Fix Unsafe-Code Policy Claims

**Files:** `README.md`, `docs/content/contributing/_index.md`, `docs/content/architecture/_index.md`, optional `docs/content/contributing/security.md`  
**Depends on:** Wave 0  
**Parallel:** yes

Implementation:

- Replace "zero unsafe" with accurate wording.
- Document that application/domain crates forbid unsafe, while audited platform interop crates opt out for GPU/Windows boundaries.
- Link to the crates that intentionally allow unsafe:
  - `hypercolor-linux-gpu-interop`
  - `hypercolor-windows-pawnio`
- Keep the claim confident, not apologetic.

Verify:

- `rg -n "zero unsafe|zero \`unsafe\`|unsafe_code = \"forbid\"" README.md docs`
- Remaining references are accurate.

## Wave 2: Release And Install Reliability

### Task 2.1: Replace Or Parameterize The Shared Release Workflow

**Files:** `.github/workflows/release.yml`, `.github/workflows/ci.yml`, scripts if needed  
**Depends on:** Wave 0  
**Parallel:** no

Implementation:

- Stop relying on a generic shared workflow that runs raw `cargo build/test/clippy --workspace`.
- Use Hypercolor's real build path: cache wrapper, workspace exclusions, Servo handling, web assets, and packaging scripts.
- Pin reusable workflows to immutable refs or vendor the workflow logic locally for v0.1.0.
- Preserve dry-run support.

Verify:

- `actionlint` if available.
- GitHub Actions dry-run or `workflow_dispatch` dry run on a test branch.
- Release job reaches artifact staging without raw workspace build drift.

### Task 2.2: Decouple GitHub Release From PyPI Publish

**Files:** `.github/workflows/ci.yml`, Python release docs  
**Depends on:** Task 2.1  
**Parallel:** yes after CI ownership is coordinated

Implementation:

- Make GitHub Release creation independent from `python-publish`.
- Either publish Python after GitHub artifacts exist, or make PyPI optional/non-blocking for v0.1.0.
- Add explicit environment/secret requirements for PyPI trusted publishing.

Verify:

- CI dependency graph shows `create-release` does not need `python-publish`.
- TestPyPI or dry-run proves the Python package path separately.
- `curl https://pypi.org/pypi/hypercolor/json` is either expected to 404 before publish or returns the intended version after publish.

### Task 2.3: Make Fresh-Clone Linux Install Work

**Files:** `README.md`, `docs/content/guide/installation.md`, `scripts/setup.sh`, `scripts/install.sh`  
**Depends on:** Wave 0  
**Parallel:** no

Implementation:

- Make README lead with `just setup` or a single canonical install command that installs prerequisites.
- Ensure `setup.sh` installs or clearly requires Node/npm before running `npm ci`.
- Do not print success after a required frontend dependency step fails.
- Decide whether `npm install` or `npm ci` is the source of truth for the UI crate.
- Ensure `trunk`, `bun`, `cargo-deny`, and wasm target setup are covered.

Verify:

- Clean Linux container or VM can run the documented path.
- `scripts/setup.sh --help` and `scripts/install.sh --help` still work.
- `bash -n scripts/setup.sh scripts/install.sh`
- A fresh clone can build UI and SDK without hidden local state.

### Task 2.4: Harden AUR Packaging

**Files:** `packaging/aur/PKGBUILD`, `.github/workflows/ci.yml`, release scripts  
**Depends on:** Task 2.1  
**Parallel:** yes

Implementation:

- Replace `sha256sums_*=('SKIP')` with real release checksums.
- Add or document the update job that patches AUR package metadata.
- Ensure release tarball paths match actual artifacts.

Verify:

- Generated PKGBUILD contains real SHA256 values.
- `namcap` if available.
- AUR package dry-run on Arch container if feasible.

### Task 2.5: Expand Contributor Verification Gates

**Files:** `CONTRIBUTING.md`, `.github/PULL_REQUEST_TEMPLATE.md`, `README.md`  
**Depends on:** Wave 0  
**Parallel:** yes

Implementation:

- Keep `just verify` for Rust, but document area-specific gates:
  - UI: `just ui-test`, `just ui-build`
  - SDK: `just sdk-lint`, `just sdk-check`, `just sdk-build`
  - Python: `just python-verify`
  - Docs: `just docs-build`, `cd docs && zola check`
  - Compat: `just compat-check`
  - E2E: `just e2e`
- Update PR template checkboxes.

Verify:

- PR template reflects the actual launch gates.
- `just --list` contains every documented recipe.

## Wave 3: Package Publication And SDK First-Run

### Task 3.1: Decide NPM Launch Strategy

**Files:** `sdk/packages/*/package.json`, SDK docs  
**Depends on:** Wave 0  
**Parallel:** no

Implementation:

- Choose one:
  - Publish `@hypercolor/sdk`, `@hypercolor/create-effect`, and `create-hypercolor-effect` before announce.
  - Or make public docs use local checkout / `file:` dependencies only until npm publish.
- If publishing, reserve package names and configure npm provenance if desired.
- If not publishing, remove `bunx create-hypercolor-effect` from first-run docs.

Verify:

- `npm view @hypercolor/sdk version`
- `npm view @hypercolor/create-effect version`
- `npm view create-hypercolor-effect version`
- Or docs no longer reference unavailable packages as the default path.

### Task 3.2: Fix Scaffolder Defaults

**Files:** `sdk/packages/create-effect/src/scaffold.ts`, tests, docs  
**Depends on:** Task 3.1  
**Parallel:** no

Implementation:

- Align default SDK dependency with the chosen launch strategy.
- If local checkout mode remains, require or default `--sdk-spec file:...`.
- Keep generated workspaces reproducible.

Verify:

- `cd sdk && bun test`
- Generated workspace installs on a clean temp directory.
- `bun run build` works in generated workspace.

### Task 3.3: Decide PyPI Launch Timing

**Files:** `.github/workflows/ci.yml`, `python/pyproject.toml`, Python docs  
**Depends on:** Task 2.2  
**Parallel:** yes

Implementation:

- Choose whether Python client ships with v0.1.0 or follows later.
- If shipping, configure trusted publishing and run TestPyPI rehearsal.
- If not shipping, make CI/docs clearly treat Python as source-only for launch.

Verify:

- TestPyPI publish or explicit disabled publish path.
- `just python-build`
- `just python-verify`

## Wave 4: Public Docs Truth Pass

### Task 4.1: Fix REST And Quick-Start Envelope Examples

**Files:** `docs/content/api/rest.md`, `docs/content/guide/quick-start.md`, Python/CLI examples if needed  
**Depends on:** Wave 0  
**Parallel:** yes

Implementation:

- Update examples to use `{ data, meta }`.
- For effect lists, use `.data.items`.
- For single resources, use `.data`.
- Mention `meta.request_id` where helpful.

Verify:

- `rg -n "jq '\\.\\[\\]|\\[\\]\\.name|data.items" docs/content`
- `just docs-build`

### Task 4.2: Rewrite Or Archive Stale Top-Level Architecture Doc

**Files:** `docs/ARCHITECTURE.md`, optional `docs/archive/`, `docs/content/architecture/_index.md`  
**Depends on:** Wave 0  
**Parallel:** yes, but coordinate with docs truth pass

Implementation:

- Remove or archive SvelteKit-era architecture.
- Remove Unix socket IPC claims.
- Remove MIT/Apache dual-license claims.
- Make Leptos, REST/WebSocket/MCP, SparkleFlinger, and current crate graph canonical.
- Create `docs/archive/` if superseded docs remain valuable.

Verify:

- `rg -n "SvelteKit|Unix Socket|MIT/Apache|hypercolor-config|320×200|320x200" docs README.md`
- `just docs-build`

### Task 4.3: Normalize Platform Support Messaging

**Files:** `README.md`, `docs/content/guide/installation.md`, `docs/content/contributing/_index.md`  
**Depends on:** Wave 0  
**Parallel:** yes

Implementation:

- Lead with Linux-first.
- Describe macOS as release/build supported only where proven.
- Describe Windows as experimental unless CI/release gates prove stronger support.
- Remove "all three platforms build clean" style claims unless backed by CI.

Verify:

- `rg -n "Linux, macOS, and Windows|all three|Windows compiles|experimental|Linux-first" README.md docs/content`
- Claims are consistent.

### Task 4.4: Fix Rust Version Drift

**Files:** README/docs/contributing/install docs  
**Depends on:** Wave 0  
**Parallel:** yes

Implementation:

- Replace Rust 1.85 references with Rust 1.94+.
- Avoid future drift by linking to `rust-toolchain.toml` or `Cargo.toml` where useful.

Verify:

- `rg -n "1\\.85|Rust 1\\." README.md docs`
- Remaining versions are intentional and current.

### Task 4.5: Fix Hardware Status Contradictions

**Files:** `docs/content/hardware/_index.md`, generated compatibility docs if needed  
**Depends on:** Task 0.2  
**Parallel:** yes

Implementation:

- Ensure Lian Li is not marked planned when supported.
- Ensure Dygma is not marketed as working until runtime behavior is gated.
- Prefer generated compatibility matrix as source of truth.

Verify:

- `just compat-check`
- `rg -n "Dygma|Lian Li|Planned|Implemented" docs/content/hardware README.md`

### Task 4.6: Make `zola check` Green

**Files:** `docs/content/hardware/compatibility.md` generator inputs or link rendering logic  
**Depends on:** Task 4.5  
**Parallel:** yes

Implementation:

- Replace vendor homepage links that return 403 during link checking.
- Use canonical support pages, omit links, or configure generated docs to avoid check-hostile links.

Verify:

- `cd docs && zola check`

### Task 4.7: Surface Security, Roadmap, And Limitations

**Files:** `README.md`, docs content  
**Depends on:** Wave 0  
**Parallel:** yes

Implementation:

- Add README links to `SECURITY.md` and `CODE_OF_CONDUCT.md`.
- Add a public launch-status/known-limitations section.
- Add or link a public roadmap that is not an internal swarm plan.

Verify:

- README includes Security, Code of Conduct, compatibility, roadmap/status, and known limitations.
- Links resolve locally.

## Wave 5: UI And Asset Polish

### Task 5.1: Remove "Next Commit" UI Copy

**Files:** `crates/hypercolor-ui/src/components/viewport_designer.rs`  
**Depends on:** Wave 0  
**Parallel:** yes

Implementation:

- Replace "lands in the next commit" copy with current-state wording.
- Hide incomplete controls if they cannot work yet.
- Keep UI text product-facing, not implementation-facing.

Verify:

- `rg -n "next commit|TODO|coming soon" crates/hypercolor-ui/src`
- `just ui-test`
- `just ui-build`

### Task 5.2: Decide Screenshot Strategy

**Files:** `effects/screenshots/`, `crates/hypercolor-ui/src/components/effect_card.rs`, release scripts/docs  
**Depends on:** Task 0.2  
**Parallel:** no

Implementation:

- Choose one:
  - Track a curated, size-bounded screenshot set.
  - Store screenshots in release assets/object storage.
  - Remove screenshot expectation from UI cards and docs for v0.1.0.
- Avoid committing hundreds of MB without an explicit asset policy.

Verify:

- `git ls-files effects/screenshots | wc -l`
- `du -sh effects/screenshots`
- UI gracefully handles missing screenshots in a clean clone.

### Task 5.3: Track JS Lockfiles Or Change Install Commands

**Files:** `.gitignore`, `sdk/bun.lock`, `crates/hypercolor-ui/package-lock.json`, CI, docs  
**Depends on:** Wave 0  
**Parallel:** yes

Implementation:

- Stop ignoring lockfiles that CI depends on, or stop using frozen/CI install commands that require absent locks.
- Prefer tracked lockfiles and frozen installs for launch reproducibility.
- Fix `crates/hypercolor-ui/package.json` license from ISC to Apache-2.0 if it remains.

Verify:

- `git check-ignore -v sdk/bun.lock crates/hypercolor-ui/package-lock.json` no longer flags intended tracked locks.
- CI uses `bun install --frozen-lockfile` or equivalent where lockfiles exist.
- `just ui-build`
- `just sdk-build`

## Wave 6: Final Verification And RC Rehearsal

### Task 6.1: Full Local Gate

**Files:** none unless failures require fixes  
**Depends on:** Waves 1-5  
**Parallel:** no

Verify:

- `just verify`
- `just compat-check`
- `just sdk-lint`
- `just sdk-check`
- `just sdk-build`
- `just ui-test`
- `just ui-build`
- `just python-verify`
- `just docs-build`
- `cd docs && zola check`
- `just deny`

### Task 6.2: Full E2E Gate

**Files:** none unless failures require fixes  
**Depends on:** Task 6.1  
**Parallel:** no

Implementation:

- Run only after explicit go-ahead because it starts daemon/browser stack.

Verify:

- `just e2e`
- Keep e2e artifacts only if debugging failures.

### Task 6.3: Release Candidate Dry Run

**Files:** release workflow/scripts only if failures require fixes  
**Depends on:** Task 6.2  
**Parallel:** no

Implementation:

- Cut an RC tag or run workflow dispatch dry-run.
- Build release tarballs and native app artifacts.
- Install from release tarball on Linux.
- Smoke CLI, daemon health, UI asset presence, bundled effects, shell completions.
- Confirm checksums and Homebrew/AUR templates.

Verify:

- RC workflow completes.
- Tarballs contain expected binaries:
  - `hypercolor-daemon`
  - `hypercolor`
  - `hypercolor-app`
  - `hypercolor-tray`
  - `hypercolor-tui`
  - `hypercolor-open`
- `hypercolor --help`
- `hypercolor-daemon --help`
- `hypercolor completions bash`
- Installed systemd/launchd files point at `hypercolor-daemon`.

### Task 6.4: Launch Review

**Files:** none unless fixes remain  
**Depends on:** Task 6.3  
**Parallel:** no

Implementation:

- Run a final multi-dimensional review focused only on changed launch-hardening work.
- Confirm no blocker/high launch findings remain.
- Confirm repo is clean and branch is ready.

Verify:

- `git status --short --branch`
- `git log --oneline --decorate -20`
- Review verdict: ready or explicit remaining caveats.

## Suggested Commit Boundaries

- `fix(security): require auth for network daemon binds`
- `fix(security): harden driver credential file permissions`
- `docs(security): document audited unsafe interop exceptions`
- `ci(release): align release workflow with hypercolor build path`
- `fix(install): make fresh clone setup deterministic`
- `ci(release): decouple github release from pypi publish`
- `build(packaging): replace aur skipped checksums`
- `docs(api): fix response envelope examples`
- `docs: refresh public architecture and platform status`
- `docs(hardware): align launch hardware status`
- `fix(ui): remove unfinished viewport copy`
- `build(sdk): make effect scaffolding launch-safe`
- `build: track frontend lockfiles for reproducible installs`
- `chore(release): add rc launch checklist`

## Critical Path

1. Wave 1 security blockers.
2. Wave 2 release/install blockers.
3. Wave 3 package publication or doc rewording.
4. Wave 4 public docs truth pass.
5. Wave 6 RC rehearsal.

Wave 5 can happen in parallel with Wave 4 once the screenshot strategy is decided.

## Stop Conditions

Pause and re-plan if:

- Non-loopback auth requires a bigger auth/session redesign than API-key enforcement.
- Release workflow needs changes in `hyperb1iss/shared-workflows`.
- Fresh-clone install exposes unsupported system dependency gaps.
- npm/PyPI package names are unavailable or require account setup that changes launch timing.
- Screenshot assets are too large for git and require CDN/object-storage infrastructure.
