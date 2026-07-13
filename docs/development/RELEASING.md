# Releasing

Releases are fully automated. One workflow dispatch produces an atomic
release commit, a tag, binaries for every platform, a GitHub Release with
AI-generated notes, and registry publishes.

## Cutting a release

1. Open **Actions → Release → Run workflow**.
2. Enter the version without the leading `v` (e.g. `0.2.0` or `0.2.0-rc.1`).
3. Leave **dry run** checked for the first pass. Review the
   `release-preview-v<version>` artifact (release notes + changelog).
4. Re-run with dry run unchecked to ship.

What the Release workflow does, in order:

1. **Validates** the version: semver shape, strictly above the latest tag,
   not already tagged, and not already published on npm or PyPI.
2. **Stamps** every version-bearing file via `scripts/set-version.ts`
   (`just set-version <v>` locally):
   - `Cargo.toml` `[workspace.package]` — every crate inherits it
   - `crates/hypercolor-app/tauri.conf.json`
   - `python/pyproject.toml` (semver prerelease translated to PEP 440:
     `-alpha.N` → `aN`, `-beta.N` → `bN`, `-rc.N` → `rcN`)
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

The CI tag lane then builds all platform artifacts, creates the GitHub
Release with the committed notes, publishes `hypercolor` +
`create-hypercolor` to npm (with provenance; prereleases go to the `next`
dist-tag), publishes the Python client to PyPI (stable only), and updates
the Homebrew tap and AUR metadata (stable only).

## Required configuration

| What | Where | Used for |
| --- | --- | --- |
| `ANTHROPIC_API_KEY` | repo secret | git-iris release notes + changelog (required) |
| `NPM_TOKEN` | repo secret | npm publishes; use a granular automation token so no OTP is needed |
| PyPI trusted publisher | pypi.org project settings | `publish-pypi` uses OIDC; register repo `hyperb1iss/hypercolor`, workflow `ci.yml` |
| `HOMEBREW_TAP_TOKEN` | repo secret | tap pushes (already configured) |
| `GIT_IRIS_MODEL` | repo variable, optional | override git-iris's default Anthropic model |

## Version alignment

`scripts/set-version.ts --verify` (or `just set-version-check <v>`) asserts
every file above carries the same version; the release workflow runs it
after stamping, and the CI `python-build` job independently rejects tags
whose pyproject version does not match.

History note: before the first automated release, published versions had
drifted (engine `0.1.0`, npm SDK `0.1.2`, scaffolder `0.1.1`, PyPI
`0.2.0a1`). Registries refuse re-published versions, so the first aligned
release must be **`0.2.0` or higher** — `0.2.0` clears npm (`> 0.1.2`) and
PyPI (`0.2.0 > 0.2.0a1`) simultaneously.

## Rehearsals

- Artifact-only rehearsal without a tag: dispatch **CI/CD** with
  `release_artifacts: full` (or `smoke` for the tarball smoke test).
- Full rehearsal without pushing: dispatch **Release** with dry run
  checked — everything is prepared and uploaded as an artifact, nothing
  leaves the runner.
