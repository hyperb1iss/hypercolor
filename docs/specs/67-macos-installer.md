# 67: macOS Installer Packaging and Release Boundary

> Captures the macOS bundle pipeline, the public OSS validation surface, and the
> private release boundary for Developer ID signing and notarization.

**Status:** Implemented. Public CI validates unsigned app packaging without
release credentials. Proprietary builds sign, notarize, accept, and promote
macOS release artifacts.
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

Local and CI builds produce per-architecture `.app` artifacts through Tauri 2's
bundler. Public CI retains them only as short-lived unsigned packaging fixtures.
The proprietary release pipeline produces the signed and notarized DMGs.

| Surface | File | Status |
|---|---|---|
| Tauri bundle config (icons, identifier, hardened runtime, DMG layout) | `crates/hypercolor-app/tauri.conf.json` | Live |
| macOS entitlements (JIT, USB, network, audio-input) | `crates/hypercolor-app/entitlements.plist` | Live |
| `Info.plist` with microphone and screen-capture purpose strings; no Apple Events string | `crates/hypercolor-app/Info.plist` | Live |
| Exact six-key daemon hardened-runtime entitlement profile | `packaging/macos/daemon.entitlements.plist` | Live |
| Sidecar staging (daemon + CLI under `target/bundle-stage/binaries/`) | `scripts/stage-app-bundle-assets.sh` | Live |
| Per-arch CI build matrix (`macos-arm64`, `macos-x64`) | `.github/workflows/ci.yml` § `build-native-app` | Live; uploads short-lived unsigned `.app` fixtures only |
| Manifest-driven Developer ID signing and notarization actor | `scripts/sign-macos-artifacts.sh` | Live; invoked only by local or proprietary builds |
| Homebrew Cask template with per-arch SHA placeholders | `packaging/homebrew/hypercolor-app.rb` | Live |
| Signed DMG and Homebrew Cask promotion | Proprietary release pipeline | Private; never receives credentials from OSS CI |

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

## 2. Signing and Notarization Boundary

Distribution outside the Mac App Store still requires an Apple-issued Developer
ID certificate and notarization service. Per [`46-cross-platform-packaging.md`
§11.2](../design/46-cross-platform-packaging.md#112-macos--apple-developer-id--notarization-required)
this requires an Apple Developer Program membership, a Developer ID identity,
an App Store Connect API key, and the repository's manifest-driven signing
actor.

Apple release credentials belong exclusively to local private keychains and
the proprietary release system. They are never configured as secrets in the
public OSS repository. Public workflows never sign, notarize, or publish a
macOS artifact.

### 2.1 One-time setup

1. Enroll in the Apple Developer Program at <https://developer.apple.com/programs/enroll/>.
2. In Keychain Access, request and download the **Developer ID Application**
   certificate (do **not** use "Mac App Distribution"; that's MAS-specific).
3. Export the cert + private key as a `.p12` with a password. Note the password.
4. Provision an App Store Connect API key at
   <https://appstoreconnect.apple.com/access/api>. Store it only in the
   proprietary release system.
5. Note the Team ID from <https://developer.apple.com/account>.

Local Apple ID notarization uses a stored `notarytool` profile. Run
`xcrun notarytool store-credentials hypercolor-notary`, then enter the Apple ID,
Team ID, and app-specific password at the secure prompts. Never pass the
password to `notarytool submit`.

### 2.2 Proprietary release inputs

The proprietary pipeline provides the signing identity, team identifier,
certificate material, and a private App Store Connect API-key file to
`scripts/sign-macos-artifacts.sh`. The API key must be a non-symlink regular
file with mode `0400` or `0600`.

The signing actor imports PKCS#12 and ephemeral-keychain passwords through a
bounded stdin frame into Security.framework. Neither password enters a process
argument. Raw Apple ID passwords are rejected. Interactive local signing may
use a stored `notarytool` keychain profile.

### 2.3 Public OSS validation

The public `build-native-app` matrix builds macOS app bundles with `--no-sign`,
checks the deployment target, and uploads the result under an `oss-ci-*` name
with seven-day retention. The release job downloads only `hypercolor-*`
artifacts, and its allowlist contains no macOS artifact type. Public CI also
runs the signing transport regression test without using real credentials.

---

## 3. Proprietary or Local Signed Build

Once the cert is in your local keychain (System keychain → "My Certificates"):

```bash
export APPLE_SIGNING_IDENTITY="Developer ID Application: Stefanie Jane (TEAMID)"
export APPLE_TEAM_ID="TEAMID"
export APPLE_API_KEY_ID="KEYID"
export APPLE_API_ISSUER="issuer-uuid"
export APPLE_API_KEY_PATH="/private/path/AuthKey_KEYID.p8"
just mac-installer --notarize
```

To use the stored local profile instead, set
`APPLE_NOTARY_KEYCHAIN_PROFILE=hypercolor-notary` and omit the three API-key
variables.

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
`xcrun notarytool log <submission-id> --keychain-profile hypercolor-notary`
and read the JSON for the offending file (typically a sidecar binary that needs
hardened-runtime entitlements applied via `codesign --deep`).

---

## 5. Cask Publication

The proprietary release pipeline templates
`packaging/homebrew/hypercolor-app.rb` with the accepted per-architecture DMG
digests and promotes the cask only after receipt validation. Public OSS CI does
not hold the tap token and cannot update the cask. `brew install --cask` reads
the promoted URLs, downloads the notarized DMG, and preserves the stapled
ticket through Homebrew's mount-and-copy flow.

---

## 6. Deferred (v1.1+)

These items are explicitly out of scope for the v1 launch per
[`46-cross-platform-packaging.md` §14](../design/46-cross-platform-packaging.md#14-phasing):

- Universal2 binary (single DMG running on both arches). Per-arch is the
  shipped path; universal2 is reachable later via
  `cargo tauri build --target universal-apple-darwin` once it earns the build
  time.
- In-app self-update. The OSS build is intentionally self-update-free.
- DMG background image. Layout positions are already configured in
  `tauri.conf.json`; adding `packaging/icons/dmg-background.png` and wiring it
  through `bundle.macOS.dmg.background` lands as a polish pass.
- Mac App Store companion. Sandboxed `device.usb` access for arbitrary RGB
  hardware is the architectural blocker.

---

## 7. Quick Reference

| Trigger | Command |
|---|---|
| Regenerate icons after editing SVG | `just mac-icons` |
| Build unsigned `.app` for local testing | `just mac-installer --profile preview` |
| Build signed + notarized DMG locally | `just mac-installer --notarize` (env vars required) |
| Check prerequisites only | `just mac-installer --check-only` |
| Validate a notarized DMG | `xcrun stapler validate <dmg>` |
| Read notary failure log | `xcrun notarytool log <id> --keychain-profile hypercolor-notary` |

---

*End of spec.*
