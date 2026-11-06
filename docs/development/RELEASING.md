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
   - the `version` requirement on every internal `hypercolor` path
     dependency, in the root `[workspace.dependencies]` and in each crate
     manifest, so published crates ask for their siblings at this release
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
Authenticode-signs the Windows installer once
[Windows signing](#windows-signing) is configured, creates the GitHub
Release with the committed notes and the Linux and Windows artifacts,
attaches the signed macOS artifacts once Apple has
notarized them, publishes `hypercolor` +
`create-hypercolor` to npm (with provenance; prereleases go to the `next`
dist-tag), publishes the Python client to PyPI (stable only), and updates
the AUR metadata (stable only).

Before it uploads the Windows installer, the tag lane installs the highest
published release older than the build on the runner and upgrades it with
the new installer twice, once clicking through the wizard and once with
`/S`. The build fails if either upgrade loses the user's autostart
setting, recreates a removed shortcut, or leaves the previous release's
files behind. [Windows Installer](WINDOWS_INSTALLER.md) describes that
upgrade contract and the pinned NSIS template behind it.

The tag lane also updates the Homebrew tap: `update-homebrew` renders
`packaging/homebrew/hypercolor.rb` and `packaging/homebrew/hypercolor-app.rb`
with `scripts/homebrew-formula.mjs`, filling the Linux amd64 and arm64
stanzas, the Apple silicon macOS stanza, and the Apple silicon cask from the
tarballs and DMG the release just published. It waits for `attach-macos`
so a notarized release updates every platform at once, but it does not need
macOS to succeed: without a notarized build, Linux still advances and macOS
carries forward, as the signing section below describes.

macOS releases target Apple silicon only. macOS 26 Tahoe is the last release
that runs on Intel Macs, so the release builds, signs, and publishes no
x86_64 macOS artifacts. The Homebrew formula and cask declare
`depends_on arch: :arm64`, and both curl installers refuse Intel Macs with a
clear message before downloading anything.

macOS artifacts are Developer ID signed and notarized on the GitHub macOS
runners in two stages. `build-native-app` compiles the sidecars and the
unsigned Tauri app bundle on an Apple silicon runner, then uploads them as an
`unsigned-macos-arm64` payload; it never holds Apple credentials. `sign-macos`
picks that payload up on a fresh runner, imports the certificate into an
ephemeral keychain, signs every binary with the hardened runtime, notarizes
and staples the app and DMG through an App Store Connect API key, assembles
and notarizes the standalone tarball, and verifies that every signature
carries `APPLE_TEAM_ID` before uploading the release artifacts.

Apple can hold a notarization for hours, so macOS never gates the rest of
the release. `create-release` publishes the Linux and Windows artifacts
without waiting for `sign-macos`, and npm, PyPI, the AUR, and Nix follow it.
`attach-macos` then adds the signed arm64 tarball, its checksum, the DMG,
and the DMG's notarization receipt to the same release once `sign-macos`
succeeds, uploading only assets the release does not already carry.
`update-homebrew` waits for `attach-macos` to finish but not to succeed: the
Linux formula advances on every stable tag, and without a notarized macOS
build the renderer carries the published macOS stanza forward and leaves the
cask alone. A release carrying only part of its macOS assets stops the job
instead. The tap's macOS stanza can also be withdrawn (`homebrew-formula.mjs
--withdraw-macos`, with `disable!` on the cask). A macOS install or upgrade
of the formula then fails with a reason instead of offering an older build,
the cask refuses installs and skips upgrades with a warning, carry mode keeps
the formula withdrawn, and the next release with a notarized build restores
macOS and the cask. If `sign-macos` times out waiting on Apple,
check **Actions → macOS Notary Status → Run workflow**, which lists the
team's submissions and their status. Once Apple has cleared the queue, use
"Re-run failed jobs" on the tag's CI/CD run: the signing job picks up the
same unsigned payload (kept for seven days) without rebuilding, and
`attach-macos` and `update-homebrew` follow. The
rerun still waits for a macOS runner and submits new notarizations. The
`release-credentials` job checks all seven
Apple secrets before any artifact job starts, and the Release workflow
refuses a non-dry run while any of them is missing.

## Signed macOS smoke checkpoint

Before the non-dry run, build signed artifacts without a tag by dispatching
**CI/CD** with `release_artifacts: full` from the release source. Leave
`release_version` blank: the artifact jobs reject any version whose base
differs from the current Cargo version, which the release commit has not
stamped yet, so the version being cut does not build. The blank default
(`<cargo version>-ci.0`) always does. Download the arm64 DMG from that run and check it on an
Apple Silicon Mac:

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

If any row fails, stop after the dry run.

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
| Windows signing              | repo variables, optional   | seven variables plus the `windows-signing` environment, all listed under [Windows signing](#windows-signing) with how to provision them                                                                   |

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
   in shell history: `openssl base64 -A -in developer-id.p12 | gh secret set APPLE_CERTIFICATE`,
   `gh secret set APPLE_API_KEY_CONTENT < AuthKey_<id>.p8`, and so on.

The Developer ID certificate is valid for five years. Rotating it means
repeating steps 1 to 3 and replacing `APPLE_CERTIFICATE` and
`APPLE_CERTIFICATE_PASSWORD`; the identity string and team ID stay the same.

After provisioning or rotating any of these secrets, run **Actions → macOS
Signing Selftest → Run workflow**. It signs, verifies, and notarizes a
throwaway binary through the same scripts the release lane uses and
reports in minutes. The same workflow runs on pull requests that touch
the signing scripts. A brand-new team's first notarizations can sit
"In Progress" for hours while Apple reviews the account; the selftest then
fails with the submission ID and its status instead of hanging.

## Windows signing

Windows signing is optional and stays off until the repository variable
`ARTIFACT_SIGNING_ACCOUNT` is set. While it is off, every release lane
logs a notice and ships the unsigned installer exactly as before. Once it
is set, the other six variables below must be set too, or
`release-credentials` fails before any build starts (and the Release
workflow refuses a non-dry run). Clearing `ARTIFACT_SIGNING_ACCOUNT` alone
turns signing off again, which is the escape hatch if Azure is down on
release day.

Signing goes through
[Azure Artifact Signing](https://learn.microsoft.com/azure/artifact-signing/)
(formerly Trusted Signing). It accepts individual developers in the United
States and Canada, costs $9.99 a month on the Basic tier for 5,000
signatures, keeps the key in Microsoft's HSMs, and authenticates CI with a
federated credential, so no signing secret exists to leak or rotate. The
certificate names the maintainer as publisher. Microsoft says EV
certificates
[no longer bypass SmartScreen](https://learn.microsoft.com/windows/apps/package-and-deploy/smartscreen-reputation),
so EV's higher price buys this project nothing. The free
[SignPath Foundation](https://signpath.org/terms) program names itself as
publisher, approves every release by hand, and cannot sign the NSIS
uninstaller in the middle of a Tauri build.

### How the release lane signs

1. The `release-credentials` job runs
   `scripts/sign-windows-artifacts.ps1 resolve` and publishes one
   decision, `windows_signing`, for the whole run. It is true only for tag
   runs on `v*` tags and full dispatches from `main`; warm runs and other
   refs stay unsigned with a notice.
2. The `build-native-app` job builds, bundles, and tests the Windows
   installer exactly as it does unsigned, including the DLL closure check
   and the in-place upgrade check. On a signing run it uploads the bundle
   inputs (`hypercolor-app.exe`, `target/bundle-stage`, and the tested
   unsigned installer) as `unsigned-windows-x64` instead of uploading the
   installer.
   That job never gets an OIDC token: npm and PyPI trust any `ci.yml` job
   that can mint one, and the build runs hundreds of third-party build
   scripts.
3. The `sign-windows` job runs on a fresh Windows runner in the
   `windows-signing` environment and compiles nothing. It signs in to Azure,
   fetches the pinned Artifact Signing dlib, and reruns the Tauri bundle
   phase (`cargo tauri bundle --no-binary-patching`) with a `signCommand`
   that points at the same script. Tauri signs the app, the daemon and CLI
   sidecars, `tools\hypercolor-smbus-service.exe`,
   `tools\hypercolor-windows-helper.exe`, the ANGLE DLLs, and its copy of
   the NSIS plugins before makensis packs them. The NSIS compiler then signs
   the uninstaller it generates through the `!uninstfinalize` hook in
   `installer.nsi`, which fails the build if that signature fails, and Tauri
   signs the finished installer last. The script also signs `nsExec.dll`, a
   stock NSIS plugin that the Hypercolor hooks call and Tauri's own list
   leaves out. Vendor binaries keep their vendors' signatures: the VC++
   runtime DLLs and `PawnIO_setup.exe`, whose hash
   `install-bundled-pawnio.ps1` checks at install time.
4. The same job unpacks the signed installer with 7-Zip and fails unless
   three things hold. Every first-party exe and dll inside it, including
   `uninstall.exe` and the NSIS plugins, carries a valid, timestamped
   signature from `WINDOWS_SIGNING_PUBLISHER`. Every VC++ runtime DLL
   carries a valid Microsoft signature, and `PawnIO_setup.exe` matches its
   pinned SHA-256. The signed installer ships the same files as the tested
   unsigned one, and every non-executable file and every vendor binary in it
   is byte for byte equal. The job then reruns the DLL closure check on the
   signed installer and uploads it under the release artifact name that
   `create-release` publishes.

A failed `sign-windows` job reruns on its own with "Re-run failed jobs",
because the unsigned payload is kept for seven days.

### Provisioning Artifact Signing

Everything except identity validation also works from the Azure CLI; the
[quickstart](https://learn.microsoft.com/azure/artifact-signing/quickstart)
covers both paths.

1. **Azure subscription.** Use a pay-as-you-go subscription under a billing
   account whose type is **Individual**, with the legal name and sold-to
   address exactly as they appear on your government ID. Free, trial, and
   sponsored subscriptions are not supported.
2. **Resource provider.** `az provider register --namespace Microsoft.CodeSigning`.
3. **Signing account.** Create a resource group, then an Artifact Signing
   account on the Basic tier, for example
   `az extension add --name artifact-signing` and
   `az artifact-signing create -n <account> -l eastus -g <group> --sku Basic`.
   Billing starts as soon as the account exists. Note the account's
   regional endpoint (`https://eus.codesigning.azure.net` for East US); a
   region and endpoint mismatch fails with 403.
4. **Identity validation.** Assign yourself the
   **Artifact Signing Identity Verifier** role on the account (Owner alone
   does not include it). In the portal, open the account's **Identity
   validations**, choose **New identity**, **Public**, **Individual**, and
   complete the Microsoft Entra Verified ID check with your government ID
   and Microsoft Authenticator. The certificate subject is your validated
   legal name with city, state, and country; it cannot be customized.
   Validation can be renewed from 60 days before it expires.
5. **Certificate profile.** Create a **Public Trust** certificate profile
   on the validated identity (not Public Trust Test, whose signatures stop
   validating when the certificate expires).
6. **CI identity.** Create an app registration and its service principal:
   `az ad app create --display-name hypercolor-windows-signing`, then
   `az ad sp create --id <appId>`. Add the federated credential GitHub's
   tokens will match:

   ```bash
   az ad app federated-credential create --id <appId> --parameters '{
     "name": "github-windows-signing",
     "issuer": "https://token.actions.githubusercontent.com",
     "subject": "repo:hyperb1iss/hypercolor:environment:windows-signing",
     "audiences": ["api://AzureADTokenExchange"]
   }'
   ```

   Subjects match case-sensitively. If the repository has opted in to
   GitHub's
   [immutable subject claims](https://learn.microsoft.com/entra/workload-id/workload-identities-github-immutable-subjects),
   use the `repo:<owner>@<owner_id>/<repo>@<repo_id>:environment:windows-signing`
   form that `azure/login` prints in its log instead.
7. **Signing role.** Give the CI identity
   **Artifact Signing Certificate Profile Signer**, scoped to the profile:
   `az role assignment create --assignee <appId> --role "Artifact Signing Certificate Profile Signer" --scope /subscriptions/<subscription>/resourceGroups/<group>/providers/Microsoft.CodeSigning/codeSigningAccounts/<account>/certificateProfiles/<profile>`.
8. **GitHub environment.** In **Settings → Environments**, create
   `windows-signing`. Under **Deployment branches and tags**, choose
   **Selected branches and tags** and add the branch rule `main` and the
   tag rule `v*`. The environment holds no secrets; it exists so the
   federated credential trusts only runs GitHub let into it.
9. **Repository variables.** In **Settings → Secrets and variables →
   Actions → Variables**, or with `gh variable set <name> --body <value>`,
   set these seven in order. Set `ARTIFACT_SIGNING_ACCOUNT` last, because
   it switches signing on. They are identifiers, not credentials; the
   federated credential is what grants access.

   | Variable                               | Value                                                                       |
   | -------------------------------------- | --------------------------------------------------------------------------- |
   | `AZURE_CLIENT_ID`                      | the app registration's application (client) ID                              |
   | `AZURE_TENANT_ID`                      | the Entra tenant ID                                                         |
   | `AZURE_SUBSCRIPTION_ID`                | the subscription that holds the signing account                             |
   | `ARTIFACT_SIGNING_ENDPOINT`            | the account's regional endpoint, e.g. `https://eus.codesigning.azure.net`   |
   | `ARTIFACT_SIGNING_CERTIFICATE_PROFILE` | the certificate profile name                                                |
   | `WINDOWS_SIGNING_PUBLISHER`            | the certificate's common name, your validated legal name exactly            |
   | `ARTIFACT_SIGNING_ACCOUNT`             | the signing account name; setting it turns signing on                       |

10. **Selftest.** Run **Actions → Windows Signing Selftest → Run workflow**
    from `main`. It signs a throwaway binary through the release script,
    once directly and once through the command line makensis uses for the
    uninstaller, and prints the signer it got. If that signer differs from
    `WINDOWS_SIGNING_PUBLISHER`, the run fails and names the actual one.
    Run it again after renewing the identity validation, changing the
    federated credential or role, or editing
    `scripts/sign-windows-artifacts.ps1`.

### Signed Windows smoke checkpoint

Before the first signed release, dispatch **CI/CD** with
`release_artifacts: full` from `main` and check:

- the `sign-windows` job passes, and the summary of its "Verify Windows
  signatures" step lists every payload binary with your name as signer,
  the VC++ runtime with Microsoft, and `PawnIO_setup.exe` with its vendor;
- on a clean Windows 11 machine, the installer downloaded through a
  browser shows your name as the verified publisher in SmartScreen (which
  may still warn until the publisher builds reputation) and in the UAC
  prompt, and its **Properties → Digital Signatures** tab shows a
  timestamp;
- after installing, `Get-AuthenticodeSignature` reports `Valid` for every
  exe under the install directory and `tools\`, and for `uninstall.exe`;
- hardware setup installs PawnIO, the app starts, and uninstalling from
  **Settings → Apps** works.

### After the first signed release

The public docs still say Windows builds are unsigned. Once a signed
release has shipped, replace those sentences in `docs/content/download.md`,
`docs/content/guide/installation.md`, and
`docs/content/guide/choose-your-install.md` with the publisher name, and
keep a note that SmartScreen may warn until the new publisher has
reputation.

### Maintenance

- No secret expires. Certificates renew daily inside Azure and last 72
  hours, which is why every signature carries a timestamp from
  `http://timestamp.acs.microsoft.com`; signed releases stay valid after
  the certificate that signed them expires.
- The identity validation does expire. Renew it from the portal, then run
  the selftest.
- The signing script (`scripts/sign-windows-artifacts.ps1`) pins the
  `Microsoft.ArtifactSigning.Client` dlib by version and SHA-256. Bump both
  together, then run the selftest.
- The NSIS plugins the template calls beyond Tauri's own list are named in
  the script's `$ExtraNsisPlugins`. A new plugin call that is missing there
  fails verification with the unsigned DLL's path.

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
- Unsigned rehearsal that also warms the release cache: dispatch **CI/CD**
  from `main` with `release_artifacts: warm`, or **Release Cache Warm**, which
  does the same every night. Warm builds every platform's release artifacts
  without signing or the normal CI lanes, and because it runs on `main` it
  writes those compiles to R2, so the next tag's release builds read them
  instead of compiling the release profile cold. Dispatch it by hand after a
  lockfile or toolchain change when a release is due before the next night.
- Full rehearsal without pushing: dispatch **Release** with dry run
  checked. Everything is prepared and uploaded as an artifact, and nothing
  leaves the runner.
