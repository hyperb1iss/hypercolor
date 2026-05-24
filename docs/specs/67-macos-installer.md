# 67 — macOS Installer: Wired State and Signing Flip-The-Switch

> Captures the macOS bundle pipeline as it stands today, the discrete pieces of
> hardware-key infrastructure required to ship a Developer ID notarized DMG, and
> the exact patches to apply once the Apple credentials are provisioned.

**Status:** Active — local + CI scaffolding wired, signing/notarization deferred until creds exist
**Scope:** `scripts/build-mac-installer.sh`, `scripts/generate-mac-icons.sh`,
`crates/hypercolor-app/icons/`, `crates/hypercolor-app/tauri.conf.json`,
`.github/workflows/ci.yml` (mac branches of `build-native-app`)
**Author:** Nova
**Date:** 2026-05-24
**Companion to:** [`docs/design/46-cross-platform-packaging.md`](../design/46-cross-platform-packaging.md),
[`docs/specs/61-packaging-release-hardening.md`](61-packaging-release-hardening.md)

---

## 1. What's Wired Today

### 1.1 Bundle assembly

Local and CI builds both produce per-arch DMG + `.app` artifacts via Tauri 2's bundler.

| Surface | File | Status |
|---|---|---|
| Tauri bundle config (icons, identifier, hardened runtime, DMG layout) | `crates/hypercolor-app/tauri.conf.json` | Live |
| macOS entitlements (JIT, USB, network, audio-input) | `crates/hypercolor-app/entitlements.plist` | Live |
| `Info.plist` with NSMicrophoneUsageDescription + NSAppleEventsUsageDescription | `crates/hypercolor-app/Info.plist` | Live |
| Sidecar staging (daemon + CLI under `target/bundle-stage/binaries/`) | `scripts/stage-app-bundle-assets.sh` | Live |
| Per-arch CI build matrix (`macos-arm64`, `macos-x64`) | `.github/workflows/ci.yml` § `build-native-app` | Live, currently `--no-sign` |
| DMG artifact name normalization to `Hypercolor-<ver>-<arch>.dmg` | `.github/workflows/ci.yml` § Normalize macOS DMG | Live |
| Homebrew Cask template with per-arch SHA placeholders | `packaging/homebrew/hypercolor-app.rb` | Live |
| Cask publish step (commits to `hyperb1iss/homebrew-tap`) | `.github/workflows/ci.yml` § `update-homebrew` | Live |

### 1.2 Local build script

`scripts/build-mac-installer.sh` mirrors `scripts/build-windows-installer.ps1`.

```bash
just mac-installer                                    # unsigned, release profile
just mac-installer --profile preview                  # faster local iteration
just mac-installer --notarize                         # sign + notarize (needs env)
just mac-installer --check-only                       # verify prerequisites
```

Prerequisites it asserts: `cargo`, `rustc`, `bun`, `trunk`, `xcrun`, `cargo-tauri`.
`cargo-tauri` is now installed by both `scripts/setup.sh` and `scripts/setup.ps1`.

### 1.3 Icon ladder

`scripts/generate-mac-icons.sh` rasterizes `packaging/icons/hypercolor.svg`
through Quick Look (WebKit-based, ships with macOS) at 1024px, downscales the
full Apple iconset (16/32/128/256/512 at @1x and @2x) via `sips`, and assembles
`icon.icns` with `iconutil`. The text wordmark is stripped from the source SVG
before rasterizing because it is illegible below 128px and macOS HIG recommends
against text inside dock icons; the Finder/Dock label already names the app.

Generated files committed under `crates/hypercolor-app/icons/`:

| File | Size | Consumer |
|---|---|---|
| `32x32.png` | 32×32 | Tauri (small) |
| `128x128.png` | 128×128 | Tauri (medium) |
| `128x128@2x.png` | 256×256 | Tauri (retina medium) |
| `icon.png` | 1024×1024 | Tauri (general/Linux) |
| `icon.icns` | full ladder | macOS bundle |
| `icon.ico` | (Windows) | Windows installer |

Re-run `just mac-icons` after editing the source SVG. Generated artifacts are
committed so contributors without Quick Look tooling can still build.

---

## 2. What Signing + Notarization Need

Distribution outside the Mac App Store still requires an Apple-issued Developer
ID certificate and notarization service. Per [`46-cross-platform-packaging.md`
§11.2](../design/46-cross-platform-packaging.md#112-macos--apple-developer-id--notarization-required)
this is a $99/yr Apple Developer Program membership plus an
`apple-actions/import-codesign-certs@v3` step in CI.

### 2.1 One-time setup

1. Enroll in the Apple Developer Program at <https://developer.apple.com/programs/enroll/>.
2. In Keychain Access, request and download the **Developer ID Application**
   certificate (do **not** use "Mac App Distribution"; that's MAS-specific).
3. Export the cert + private key as a `.p12` with a password. Note the password.
4. Generate an app-specific password for the Apple ID at
   <https://appleid.apple.com/account/manage> → Sign-In and Security →
   App-Specific Passwords. (Alternatively, provision an App Store Connect API
   key at <https://appstoreconnect.apple.com/access/api> — preferred for CI.)
5. Note the Team ID from <https://developer.apple.com/account>.

### 2.2 GitHub secrets to add

In the repo settings (`Settings → Secrets and variables → Actions`):

| Secret | Value |
|---|---|
| `APPLE_DEVELOPER_ID_P12` | Base64 of the exported `.p12` (`base64 -i cert.p12 | pbcopy`) |
| `APPLE_DEVELOPER_ID_P12_PASSWORD` | The password used during `.p12` export |
| `APPLE_SIGNING_IDENTITY` | The full identity string, e.g. `Developer ID Application: Stefanie Jane (TEAMID)` |
| `APPLE_ID` | The Apple ID email used for the Developer Program |
| `APPLE_TEAM_ID` | The 10-character team identifier |
| `APPLE_APP_SPECIFIC_PASSWORD` | The app-specific password from step 4 |

If using App Store Connect API keys instead, swap the last three for
`APPLE_API_KEY_ID`, `APPLE_API_ISSUER`, and `APPLE_API_KEY_PATH`.

### 2.3 CI patch (drop-in for `.github/workflows/ci.yml`)

In the `build-native-app` job, gate signing/notarization to tag builds with
the matrix entries that have `cask_arch != ""`. Add these steps before the
existing **Build Tauri native bundle** step:

```yaml
- name: Import Apple Developer ID certificate
  if: matrix.cask_arch != '' && startsWith(github.ref, 'refs/tags/')
  uses: apple-actions/import-codesign-certs@v3
  with:
    p12-file-base64: ${{ secrets.APPLE_DEVELOPER_ID_P12 }}
    p12-password: ${{ secrets.APPLE_DEVELOPER_ID_P12_PASSWORD }}
```

Update the **Build Tauri native bundle** step to drop `--no-sign` on signed
runs and to surface the signing identity:

```yaml
- name: Build Tauri native bundle
  working-directory: crates/hypercolor-app
  shell: pwsh
  env:
    TAURI_BUNDLES: ${{ matrix.bundles }}
    APPLE_SIGNING_IDENTITY: ${{ secrets.APPLE_SIGNING_IDENTITY }}
  run: |
    $configArgs = @()
    if (Test-Path "tauri.bundle.conf.json") {
      $configArgs += @("--config", "tauri.bundle.conf.json")
    }
    if ($env:RUNNER_OS -eq "Windows" -and (Test-Path "tauri.windows.bundle.conf.json")) {
      $configArgs += @("--config", "tauri.windows.bundle.conf.json")
    }
    $signArgs = @()
    if ($env:RUNNER_OS -ne "macOS" -or [string]::IsNullOrEmpty($env:APPLE_SIGNING_IDENTITY)) {
      $signArgs += @("--no-sign")
    }
    cargo tauri build --ci @signArgs --bundles $env:TAURI_BUNDLES @configArgs
```

Add a notarization step after **Normalize macOS DMG artifact name**, before
**Upload native app bundle**:

```yaml
- name: Notarize and staple macOS DMG
  if: matrix.cask_arch != '' && startsWith(github.ref, 'refs/tags/')
  env:
    APPLE_ID: ${{ secrets.APPLE_ID }}
    APPLE_TEAM_ID: ${{ secrets.APPLE_TEAM_ID }}
    APPLE_APP_SPECIFIC_PASSWORD: ${{ secrets.APPLE_APP_SPECIFIC_PASSWORD }}
  run: |
    set -euo pipefail
    dmg="$(find target/release/bundle/dmg crates/hypercolor-app/target/release/bundle/dmg \
            -maxdepth 1 -type f -name '*.dmg' 2>/dev/null | head -1)"
    [ -n "${dmg}" ] || { echo 'no DMG found for notarization' >&2; exit 1; }
    xcrun notarytool submit "${dmg}" \
      --apple-id "${APPLE_ID}" \
      --team-id "${APPLE_TEAM_ID}" \
      --password "${APPLE_APP_SPECIFIC_PASSWORD}" \
      --wait --timeout 30m
    xcrun stapler staple "${dmg}"
    xcrun stapler validate "${dmg}"
```

Untagged builds and PRs keep building unsigned DMGs as today.

---

## 3. Local Signed Build

Once the cert is in your local keychain (System keychain → "My Certificates"):

```bash
export APPLE_SIGNING_IDENTITY="Developer ID Application: Stefanie Jane (TEAMID)"
export APPLE_ID="stef@hyperbliss.tech"
export APPLE_TEAM_ID="TEAMID"
export APPLE_APP_SPECIFIC_PASSWORD="xxxx-xxxx-xxxx-xxxx"
just mac-installer --notarize
```

The script auto-detects which env vars are present and only invokes notary when
asked. Unsigned local builds remain a one-liner for dev iteration:

```bash
just mac-installer --profile preview --skip-ui --skip-effects
```

---

## 4. Verification

After a signed + notarized build, validate the resulting DMG on a clean Mac
that has never seen the developer keychain:

```bash
# Ticket should be stapled to the DMG itself
xcrun stapler validate Hypercolor-*-arm64.dmg

# Gatekeeper policy assessment
spctl --assess --type install --verbose Hypercolor-*-arm64.dmg

# Inspect the bundle once mounted
codesign --verify --deep --strict --verbose=2 "/Volumes/Hypercolor/Hypercolor.app"
```

A successful run prints `the validate action worked!`, `accepted`, and
`valid on disk` respectively. If `spctl` reports `rejected (rejected source=no
usable signature)` or notary returns `Invalid`, fetch the log with
`xcrun notarytool log <submission-id> --apple-id ... --team-id ... --password ...`
and read the JSON for the offending file (typically a sidecar binary that needs
hardened-runtime entitlements applied via `codesign --deep`).

---

## 5. Cask Publication

The `update-homebrew` CI job already templates `packaging/homebrew/hypercolor-app.rb`
with the per-arch DMG SHA256s and commits the result to `hyperb1iss/homebrew-tap`
under `Casks/hypercolor-app.rb`. No additional work is required for cask
distribution once the DMG is notarized — `brew install --cask` reads the URLs
from the cask formula, downloads the notarized DMG, and Homebrew's mount-and-copy
flow inherits the staple from the DMG.

---

## 6. Deferred (v1.1+)

These items are explicitly out of scope for the v1 launch per
[`46-cross-platform-packaging.md` §14](../design/46-cross-platform-packaging.md#14-phasing):

- Universal2 binary (single DMG running on both arches). Per-arch is the
  shipped path; universal2 is reachable later via
  `cargo tauri build --target universal-apple-darwin` once it earns the build
  time.
- `tauri-plugin-updater` integration for in-app self-update. Sparkle-equivalent
  flow; design lives in [`docs/design/50-update-pipeline.md`](../design/50-update-pipeline.md).
- DMG background image. Layout positions are already configured in
  `tauri.conf.json`; adding `packaging/icons/dmg-background.png` and wiring it
  through `bundle.macOS.dmg.background` lands as a polish pass.
- Mac App Store companion. Sandboxed `device.usb` access for arbitrary RGB
  hardware is the architectural blocker; a network-drivers-plus-cloud-remote
  companion is a separate product surface and tracked under
  [`docs/design/48-hypercolor-remote.md`](../design/48-hypercolor-remote.md).

---

## 7. Quick Reference

| Trigger | Command |
|---|---|
| Regenerate icons after editing SVG | `just mac-icons` |
| Build unsigned DMG for local testing | `just mac-installer --profile preview` |
| Build signed + notarized DMG locally | `just mac-installer --notarize` (env vars required) |
| Check prerequisites only | `just mac-installer --check-only` |
| Validate a notarized DMG | `xcrun stapler validate <dmg>` |
| Read notary failure log | `xcrun notarytool log <id> --apple-id ... --team-id ... --password ...` |

---

*End of spec.*
