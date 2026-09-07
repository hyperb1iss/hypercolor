# hypercolor-openrgb-host

*Host-side OpenRGB integration: is it installed, is it answering, how do you
install it here, what permissions are missing, and how do we run it headless.*

This crate backs Spec 81 layer 3. The desktop app supervisor and the
`hypercolor openrgb` CLI verb call it before leaning on the OpenRGB fallback
bridge (`hypercolor-driver-openrgb`). It follows the platform-crate pattern:
every type is neutral and `Serialize`/`Deserialize` on every target, and only
the functions that touch the OS are gated behind `cfg(target_os)` inside this
crate. Stubs return `None` or an empty `Vec` elsewhere, so the app and the CLI
compile identically on Linux, Windows, and macOS.

## Workspace position

**Depends on:** `hypercolor-openrgb-sdk` (server probe),
`hypercolor-persistence` (durable config replace), `serde`, `serde_json`,
`toml`, `tokio`, `thiserror`, `tracing`. Never `hypercolor-core` or the
daemon.

**Depended on by:** `hypercolor-app`, `hypercolor-cli`.

## API

Anything that spawns a process or opens a socket is `async` on tokio, matching
the SDK client. Filesystem inspection and the pure builders are synchronous.

| Entry point | What it answers |
| --- | --- |
| `detect_binary() -> Option<OpenRgbBinary>` | PATH walk (`openrgb`/`OpenRGB`), platform install locations, portable AppImages, then `flatpak info org.openrgb.OpenRGB`. Version via `--version` behind a 3 s timeout. |
| `probe_server(addr, timeout) -> ServerProbe` | SDK handshake plus controller count. Never fails; unreachable servers come back with `reachable: false` and the error text. |
| `install_hints() -> Vec<InstallHint>` | Detects pacman/apt/dnf/zypper/flatpak on PATH and returns exact commands in preference order. `install_hints_for(platform, &[InstallMethod])` is the pure selector. |
| `permission_checks(binary) -> Vec<PermissionCheck>` | Linux only: udev rules present, `i2c-dev` loaded, `/dev/i2c-*` and `/dev/hidraw*` writable. Each failure carries the remedy command. `linux_permission_checks_at(root, binary)` runs against an injectable root. |
| `managed_config_dir(base_data_dir) -> ManagedConfigDir` | `<data>/openrgb`, the directory passed to OpenRGB's `--config`. |
| `write_detector_partition(dir, disabled_prefixes, known_detectors) -> Result<DetectorPartition>` | Rewrites `Detectors.detectors` in `OpenRGB.json`, preserving every other key, with a durable replace. |
| `detector_prefixes_for_drivers(driver_ids) -> Vec<String>` | Prefix lookup backed by the embedded `data/openrgb/detectors.toml`. |
| `server_command(binary, dir, port) -> ProcessSpec` | `--server --server-host 127.0.0.1 --server-port <port> --noautoconnect --config <dir> --loglevel 4`; Flatpak wraps it in `flatpak run --filesystem=<dir> org.openrgb.OpenRGB`. |

### Types

- `OpenRgbBinary { path, kind: Native | Flatpak | AppImage, version }`. For
  Flatpak, `path` is the `flatpak` launcher and the app id travels in the
  process arguments.
- `ServerProbe { reachable, protocol_version, controller_count, error }`.
- `InstallHint { platform, method, command, note }` with `Platform` and
  `InstallMethod` enums.
- `PermissionCheck { id, ok, detail, remedy }`. Ids are the `CHECK_*`
  constants.
- `ProcessSpec { program, args, env, cwd }`.
- `ManagedConfigDir { root }` with `config_path()`.
- `DetectorFamily { driver_id, prefixes, detectors }` and
  `DetectorPartition { disabled, enabled }`.

## Detector partition semantics

The universe of detector names is the union of the existing file's map, the
caller's `known_detectors`, and the embedded family seed lists. For each name:

1. a match on a disabled prefix (case-insensitive) writes `false`;
2. otherwise a match on any Hypercolor-owned prefix writes `true`, so a family
   Hypercolor stops claiming is handed back to OpenRGB;
3. otherwise the existing value is preserved, defaulting to `true`.

Unrelated user toggles (Gigabyte, MSI, ASRock, anything Hypercolor has no
native driver for) are never touched. Invalid JSON or a non-object
`Detectors` section is an error, never clobbered.

## Data

`data/openrgb/detectors.toml` maps each native driver id to the OpenRGB
detector prefixes it owns plus a seed list of known detector names. It is
embedded with `include_str!`, so shipping binaries carry no data file. Add a
`[[family]]` entry when a native driver gains OpenRGB overlap.

## Platform notes

- **Linux:** distro packages install `/usr/lib/udev/rules.d/60-openrgb.rules`;
  AppImage, Flatpak, and self-built binaries need
  `--generate-udev-rules` (or `--print-udev-rules` for Flatpak). SMBus needs
  `i2c-dev` plus `i2c-i801` (Intel) or `i2c-piix4` (AMD).
- **Windows:** HID works unelevated. SMBus needs PawnIO (OpenRGB 1.0rc2+
  dropped WinRing0) and Administrator or a service install. Hypercolor's
  native Windows SMBus path already uses PawnIO.
- **macOS:** HID only, no SMBus.

The managed server always binds `127.0.0.1`: the SDK has no authentication and
OpenRGB defaults to `0.0.0.0`.
