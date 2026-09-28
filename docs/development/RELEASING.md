# Releasing

Releases are fully automated. One workflow dispatch produces an atomic
release commit, a tag, binaries for every platform, a GitHub Release with
AI-generated notes, and registry publishes.

## Cutting a release

1. Open **Actions → Release → Run workflow**.
2. Enter the version without the leading `v` (e.g. `0.3.0` or `0.3.0-rc.1`).
3. Leave **dry run** checked for the first pass. Review the
   `release-preview-v<version>` artifact (release notes + changelog).
4. Run the signed macOS smoke checkpoint below against a signed rehearsal
   build.
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

The CI tag lane then builds the Linux, Windows, and signed macOS artifacts,
creates the GitHub Release with the committed notes, publishes `hypercolor` +
`create-hypercolor` to npm (with provenance; prereleases go to the `next`
dist-tag), publishes the Python client to PyPI (stable only), and updates
the AUR metadata (stable only).

The tag lane also updates the Homebrew tap: `update-homebrew` renders
`packaging/homebrew/hypercolor.rb` and `packaging/homebrew/hypercolor-app.rb`
with `scripts/homebrew-formula.mjs`, filling every Linux and macOS stanza and
both cask architectures from the tarballs and DMGs the release just published.

macOS artifacts are Developer ID signed and notarized on the GitHub macOS
runners. The `release-credentials` job checks all seven Apple secrets before
any artifact job starts, so a tag lane with a missing secret fails in seconds
instead of after an hour of builds. Each macOS job imports the certificate
into an ephemeral keychain, signs every binary with the hardened runtime,
notarizes and staples the app and DMG through an App Store Connect API key,
and then verifies that every signature carries `APPLE_TEAM_ID` before the
artifact is uploaded. The Release workflow refuses a non-dry run while any of
the seven secrets is missing.

## Signed macOS smoke checkpoint

Before the non-dry run, build signed artifacts without a tag by dispatching
**CI/CD** with `release_artifacts: full` and `release_version` set to an
`-rc.0` of the version being cut. Download the arm64 DMG from that run and
check it on an Apple Silicon Mac:

- `spctl -a -vvv -t open --context context:primary-signature` on the DMG and
  `spctl -a -vvv` on the installed app both report
  `source=Notarized Developer ID`, and `xcrun stapler validate` passes on both;
- the app opens from a browser download with no unidentified-developer
  warning;
- the Screen Recording and Input Monitoring prompts name Hypercolor, and the
  grants survive a quit and relaunch;
- a screen-reactive effect renders from live capture, keyboard and pointer
  input reach an interactive effect, and quitting from the tray stops the
  daemon; and
- `hypercolor --version` from the macOS tarball runs after a browser download.

If any row fails, stop after the dry run. Intel builds get CI verification
only (signature, notarization, team ID, architecture, and deployment target)
and no hardware row, because the project has no Intel test machine.

The full Spec 76 physical matrix (signed TCC owner topology, SDR and HDR rows,
the Section 19 latency and cadence contracts, the four-hour soak, and Metal 4
qualification) remains the target once the physical-hardware harness is
automated. It is not a release gate until then.

The native and standalone artifact jobs also wait for the Python OpenAPI and
WebSocket drift checks. GitHub Release creation cannot run unless both checks
and both artifact lanes succeed.

## Required configuration

| What                         | Where                      | Used for                                                                                                                                                                                                  |
| ---------------------------- | -------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `ANTHROPIC_API_KEY`          | repo secret                | git-iris release notes + changelog (required)                                                                                                                                                             |
| npm trusted publishers       | npmjs.com package settings | `publish-npm` uses OIDC (no token, automatic provenance); register repo `hyperb1iss/hypercolor`, workflow `ci.yml` on **both** `hypercolor` and `create-hypercolor`                                       |
| PyPI trusted publisher       | pypi.org project settings  | `publish-pypi` uses OIDC; register repo `hyperb1iss/hypercolor`, workflow `ci.yml`                                                                                                                        |
| `HOMEBREW_TAP_TOKEN`         | repo secret                | `update-homebrew` pushes the rendered formula to `hyperb1iss/homebrew-tap`; a fine-grained PAT scoped to that repository with Contents read/write; the job fails loudly when it is missing or cannot push |
| `AUR_SSH_PRIVATE_KEY`        | repo secret                | `update-aur` pushes `hypercolor-bin` to the AUR over SSH; the matching public key must be registered on the AUR account (1Password: "SSH Key: hypercolor AUR CI")                                         |
| `GIT_IRIS_MODEL`             | repo variable, optional    | override git-iris's default Anthropic model                                                                                                                                                               |
| `APPLE_TEAM_ID`              | repo secret                | the ten-character Apple Developer team ID; every signature is verified against it                                                                                                                         |
| `APPLE_SIGNING_IDENTITY`     | repo secret                | the certificate's full common name, `Developer ID Application: <team name> (<team id>)`                                                                                                                   |
| `APPLE_CERTIFICATE`          | repo secret                | base64 of a PKCS#12 bundle holding the Developer ID Application certificate and its private key                                                                                                           |
| `APPLE_CERTIFICATE_PASSWORD` | repo secret                | the PKCS#12 export password                                                                                                                                                                               |
| `APPLE_API_KEY_ID`           | repo secret                | App Store Connect API key ID, used by `notarytool`                                                                                                                                                        |
| `APPLE_API_ISSUER`           | repo secret                | App Store Connect issuer ID (a UUID shown above the key list)                                                                                                                                             |
| `APPLE_API_KEY_CONTENT`      | repo secret                | the full text of the `AuthKey_<id>.p8` file, including the BEGIN and END lines                                                                                                                            |

### Provisioning the Apple credentials

Only the Account Holder can create a Developer ID certificate. Everything
below runs on any machine with OpenSSL; no Mac is needed.

1. Generate a key and signing request:
   `openssl genrsa -out developer-id.key 2048` then
   `openssl req -new -key developer-id.key -out developer-id.csr -subj "/emailAddress=<you>/CN=<name>/C=US"`.
2. In the Apple Developer portal, open **Certificates → +**, choose
   **Developer ID Application** with the **G2 Sub-CA**, upload the CSR, and
   download `developerID_application.cer`.
3. Build the PKCS#12 bundle with 3DES and a SHA-1 MAC, carrying the
   [Developer ID G2 intermediate](https://www.apple.com/certificateauthority/DeveloperIDG2CA.cer)
   so the chain is complete. OpenSSL 3 defaults to AES and PBKDF2, which
   `SecItemImport` on the runners can reject. Convert both DER files to PEM
   with `openssl x509 -inform DER -in <file>.cer -out <file>.pem`, then run
   `openssl pkcs12 -export -inkey developer-id.key -in developer-id.pem -certfile DeveloperIDG2CA.pem -keypbe PBE-SHA1-3DES -certpbe PBE-SHA1-3DES -macalg sha1 -out developer-id.p12`.
   The certificate subject's CN is `APPLE_SIGNING_IDENTITY` and its OU is
   `APPLE_TEAM_ID`. `openssl verify` reports an unhandled critical extension
   on the leaf; that is Apple's private Developer ID marker, and
   `-ignore_critical` confirms the chain.
4. In App Store Connect, open **Users and Access → Integrations → Team
   Keys**, generate a key with the **Developer** role, and download the
   `.p8`. Apple offers the download exactly once.
5. Store the key, CSR, certificate, PKCS#12 bundle, its password, and the
   `.p8` in 1Password, then set the secrets from the files so no value lands
   in shell history: `base64 -w0 developer-id.p12 | gh secret set APPLE_CERTIFICATE`,
   `gh secret set APPLE_API_KEY_CONTENT < AuthKey_<id>.p8`, and so on.

The Developer ID certificate is valid for five years. Rotating it means
repeating steps 1 to 3 and replacing `APPLE_CERTIFICATE` and
`APPLE_CERTIFICATE_PASSWORD`; the identity string and team ID stay the same.

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
