# Development Environment Setup

Per-platform setup for building and running Hypercolor from source. The
common workflow (`just verify`, per-area gates, PR expectations) lives in
[CONTRIBUTING.md](../../CONTRIBUTING.md); this document covers what each
operating system needs before those commands work.

## The one-command path

```bash
just setup
```

`scripts/setup.sh` (and `scripts/setup.ps1` on Windows) is the supported
bootstrap. It installs system packages through apt, dnf, pacman, or Homebrew
depending on the host, installs rustup and the pinned toolchain, adds the
`wasm32-unknown-unknown` target to every installed toolchain, installs `just`,
`trunk`, `cargo-deny`, `tauri-cli`, and optionally `sccache` (preferring
`cargo-binstall` when present), installs Bun, and runs `bun install` in both
`crates/hypercolor-ui` and `sdk/`. It is idempotent, so re-running is safe.

Useful flags, all passed straight through:

- `-y` skips the confirmation prompts.
- `--no-system` skips system packages, so no sudo is needed.
- `--minimal` stops after the Rust toolchain and wasm target.
- `--with-servo` adds the extra dependencies the Servo HTML renderer needs.

The rest of this document is the manual fallback: what each operating system
needs when you would rather install it yourself, plus the platform-specific
setup the script deliberately leaves alone.

## All platforms

- **Rust** via [rustup](https://rustup.rs). `rust-toolchain.toml` pins the
  toolchain (currently 1.95.0); rustup installs it automatically on first
  build. Edition 2024, minimum supported Rust 1.94.
- **[just](https://github.com/casey/just)**, the command runner every
  workflow goes through.
- **[Bun](https://bun.sh)** for the TypeScript effect SDK and bundled
  effects (`just sdk-install` once, then `just effects-build`).
- **Trunk** and the `wasm32-unknown-unknown` target for the web UI
  (`just ui-dev` prints what it needs if something is missing).

Every `just` build routes through `scripts/cargo-cache-build.sh`, which
wires up sccache or ccache automatically when installed. Neither is
required, but Servo builds are dramatically faster with a warm cache; see
[SERVO_BUILD_CACHING.md](SERVO_BUILD_CACHING.md).

## Linux

Install the system libraries the daemon, app shell, and Servo renderer
link against. On Debian/Ubuntu:

```bash
sudo apt install \
  build-essential pkg-config cmake nasm clang lld \
  libudev-dev libusb-1.0-0-dev libhidapi-dev \
  libasound2-dev libpulse-dev libpipewire-0.3-dev libssl-dev \
  libxdo-dev libgtk-3-dev libwebkit2gtk-4.1-dev \
  libayatana-appindicator3-dev librsvg2-dev
```

That set matches what `scripts/setup.sh` installs, so `just setup` covers it.
The last two lines (GTK, WebKitGTK, appindicator, rsvg) are only needed for the
desktop app shell and tray; daemon-only work can skip them. `clang` and `lld`
are load-bearing: the build wrapper selects them for linking.

Two more worth having: `jq`, which `just verify` needs for the
`api-doc-route-check` gate, and `ccache`, which the build wrapper picks up
automatically when installed. Neither is installed by `just setup`.

Building the Servo HTML renderer needs more on top. Passing `--with-servo` to
the setup script installs the same set:

```bash
sudo apt install \
  gperf libegl1 libxcb1-dev libxkbcommon-dev libxkbcommon-x11-dev
```

USB and HID device access needs the udev rules:

```bash
just udev-install
```

The recipe runs `sudo` on each line itself, so do not prefix it with `sudo`.
Doing so also breaks a `just` installed through cargo or proto, because root's
PATH will not find it.

Screen capture uses the Wayland XDG portal and prompts per session; no
extra setup.

## macOS

- **Xcode Command Line Tools** (`xcode-select --install`). Full Xcode is
  not required.
- Minimum deployment target is macOS 15.2 (Sequoia).

### Code signing for local bundles

`just app-bundle` signs the app with the identity from
`APPLE_SIGNING_IDENTITY`, falling back to a local certificate named
**Hypercolor Dev**, and finally to ad-hoc signing.

The fallback matters because of how macOS permissions work. TCC keys
Screen Recording and Input Monitoring grants to the app's code-signing
identity. An ad-hoc signature has a per-build identity (its designated
requirement is the build's cdhash), so every rebuild is a brand-new app
to macOS: System Settings keeps showing the old build's toggle as
enabled while the new build reads `not_determined` and has to be granted
again. A certificate-anchored signature is stable across builds, so
grants stick.

One-time setup:

1. Open **Keychain Access**, then from the menu bar choose
   **Keychain Access → Certificate Assistant → Create a Certificate**.
2. Name: `Hypercolor Dev`. Identity type: Self-Signed Root.
   Certificate type: **Code Signing**. Create.
3. Trust it for code signing (macOS asks for your password):

   ```bash
   security find-certificate -c "Hypercolor Dev" -p > /tmp/hypercolor-dev.pem
   security add-trusted-cert -p codeSign \
     -k ~/Library/Keychains/login.keychain-db /tmp/hypercolor-dev.pem
   ```

4. Verify: `security find-identity -v -p codesigning` lists
   `"Hypercolor Dev"` as a valid identity.

The first signed build pops a keychain dialog asking whether `codesign`
may use the key; choose **Always Allow**. After granting Screen
Recording or Input Monitoring to a bundle signed this way, the grants
survive rebuilds. Stale rows from earlier ad-hoc builds can be removed
in System Settings with the minus button.

Two more pieces happen automatically during `just app-bundle`, both
mirroring the release lane. The entitlements include
`disable-library-validation` because Servo links Homebrew dylibs signed
by other teams, which hardened-runtime library validation would refuse
once the bundle is certificate-signed. And
`scripts/macos-dev-postsign.sh` re-signs the daemon with the
`tech.hyperbliss.hypercolor.sidecar` identifier after the Tauri build,
because the daemon ownership handshake verifies that identity chain
between the app and its sidecar; Tauri alone would sign it with a
filename-derived identifier the handshake rejects. The sidecar profile also
holds the Apple Events Automation entitlement used by opt-in media adapters;
standalone binaries remain ineligible.

Release DMGs use a real Developer ID plus notarization through
`just mac-installer`; see [RELEASING.md](RELEASING.md). The dev
certificate is for local iteration only and never ships.

### Permission model

The daemon asks for Microphone, Screen Recording, or Input Monitoring
only when the matching feature is enabled. After a grant, macOS
requires a process restart before capture APIs see it; the Settings
page offers that restart when it applies.

## Windows

- **Visual Studio Build Tools** with the C++ workload (MSVC linker), or a
  full Visual Studio install.
- **WebView2 runtime** for the app shell. Preinstalled on Windows 11;
  the packaged installer bootstraps it on Windows 10.
- The build, test, and run recipes carry `[windows]` variants that route
  through `scripts/cargo-cache-build.ps1`; 41 of the 128 recipes have one. The
  rest either run unchanged on both platforms or are Unix-only, so check
  `just --list` before assuming a recipe exists here.

Optional hardware support:

- Motherboard and DRAM RGB (SMBus) goes through the PawnIO kernel
  driver and a broker service. `scripts/install-windows-hardware-support.ps1`
  installs both; the individual scripts it wraps live next to it in
  `scripts/`.
- Running the daemon as a Windows service uses
  `scripts/install-windows-service.ps1`. Keyboard and mouse capture
  requires an interactive session, so prefer the foreground app while
  developing input features.

## Smoke test

Any platform, once set up:

```bash
just verify     # fmt + lint + test
just daemon     # daemon on :9420
just ui-dev     # web UI on :9430, proxying to the daemon
```

macOS and Windows app-shell work uses `just app` for the iteration
build and `just app-bundle` for a native bundle.
