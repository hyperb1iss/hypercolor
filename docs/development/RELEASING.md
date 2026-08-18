# Releasing

Releases are fully automated. One workflow dispatch produces an atomic
release commit, a tag, binaries for every platform, a GitHub Release with
AI-generated notes, and registry publishes.

## Cutting a release

1. Open **Actions → Release → Run workflow**.
2. Enter the version without the leading `v` (e.g. `0.3.0` or `0.3.0-rc.1`).
3. Leave **dry run** checked for the first pass. Review the
   `release-preview-v<version>` artifact (release notes + changelog).
4. Complete the signed macOS acceptance checkpoint below.
5. Re-run with dry run unchecked to ship.

What the Release workflow does, in order:

1. **Validates** the version: semver shape, strictly above the latest tag,
   not already tagged, and not already published on npm or PyPI.
2. **Stamps** every version-bearing file via `scripts/set-version.ts`
   (`just set-version <v>` locally):
   - `Cargo.toml` `[workspace.package]`: every workspace crate inherits it
   - `crates/hypercolor-ui/Cargo.toml`: workspace-excluded (standalone WASM
     build), so it carries its own stamped version
   - `crates/hypercolor-app/tauri.conf.json`
   - `python/pyproject.toml` (semver prerelease translated to PEP 440:
     `-alpha.N` → `aN`, `-beta.N` → `bN`, `-rc.N` → `rcN`)
   - `python/src/hypercolor/__init__.py`: the runtime `__version__`, stamped
     in the same PEP 440 form as pyproject
   - `packaging/aur/PKGBUILD` (stable releases only)
   - `sdk/packages/core/package.json`, `sdk/packages/create-effect/package.json`
3. **Refreshes lockfiles**: `cargo update --workspace`, `bun install`
   (sdk), `uv lock` (python).
4. **Generates notes with git-iris**: `.github/release-notes/v<version>.md`
   (becomes the GitHub Release body) and a `CHANGELOG.md` update.
5. **Commits atomically** (`release: v<version>`), tags, pushes both.
6. **Dispatches ci.yml on the tag.** This is explicit because tags pushed
   with `GITHUB_TOKEN` never fire `on: push` workflows; the tag-lane jobs
   in ci.yml accept `workflow_dispatch` for exactly this reason.

The CI tag lane then builds the Linux and Windows artifacts, creates the
GitHub Release with the committed notes, publishes `hypercolor` +
`create-hypercolor` to npm (with provenance; prereleases go to the `next`
dist-tag), publishes the Python client to PyPI (stable only), and updates
the AUR metadata (stable only).

Public CI ships no macOS artifacts and does not update the Homebrew tap:
macOS binaries require Developer ID signing that repository runners cannot
perform, so signed macOS artifacts are produced and attached through the
signed acceptance checkpoint below, and tap updates are manual until a
signing-capable release lane exists.

## Signed macOS acceptance checkpoint

Spec 76 acceptance is a manual release checkpoint until the physical-hardware
harness is automated. Before shipping a release that includes macOS screen
capture or host input changes, run the signed packaged release candidate on
the required Apple Silicon and Intel hardware and retain one acceptance bundle
covering:

- the signed TCC owner matrix and selected capability topology, including the
  broker decision;
- keyboard, pointer, SDR, HDR, picker, lifecycle, and teardown acceptance for
  the rows supported by each machine;
- the Section 19 latency, cadence, zero-copy, byte-reconciliation, and
  30-minute results, plus the Section 18.5 four-hour combined soak; and
- one Metal 4 qualification and adoption artifact for every active device that
  exposes the required facilities.

Record the immutable artifact location and checksum in the release checklist.
CI fixtures, unsigned local runs, and a successful build do not replace this
evidence. If the signed bundle does not exist or any required row fails, stop
after the dry run. The repository does not currently contain a completed
physical-acceptance bundle.

The native and standalone artifact jobs also wait for the Python OpenAPI and
WebSocket drift checks. GitHub Release creation cannot run unless both checks
and both artifact lanes succeed.

## Required configuration

| What | Where | Used for |
| --- | --- | --- |
| `ANTHROPIC_API_KEY` | repo secret | git-iris release notes + changelog (required) |
| npm trusted publishers | npmjs.com package settings | `publish-npm` uses OIDC (no token, automatic provenance); register repo `hyperb1iss/hypercolor`, workflow `ci.yml` on **both** `hypercolor` and `create-hypercolor` |
| PyPI trusted publisher | pypi.org project settings | `publish-pypi` uses OIDC; register repo `hyperb1iss/hypercolor`, workflow `ci.yml` |
| `HOMEBREW_TAP_TOKEN` | repo secret | currently unused; retained for the future signing-capable tap lane |
| `GIT_IRIS_MODEL` | repo variable, optional | override git-iris's default Anthropic model |

## Version alignment

`scripts/set-version.ts --verify` (or `just set-version-check <v>`) asserts
every file above carries the same version; the release workflow runs it
after stamping, and the CI `python-build` job independently rejects tags
whose pyproject version does not match.

Every version-bearing file now tracks the same number, so the only floor a
new release has to clear is the latest tag, which validation enforces in
step 1.

## Rehearsals

- Artifact-only rehearsal without a tag: dispatch **CI/CD** with
  `release_artifacts: full` (or `smoke` for the tarball smoke test).
- Full rehearsal without pushing: dispatch **Release** with dry run
  checked. Everything is prepared and uploaded as an artifact, and nothing
  leaves the runner.
