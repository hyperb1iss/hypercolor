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

**Intended consumers:** `hypercolor-app` (supervisor plan and Tauri commands)
and `hypercolor-cli` (`hypercolor openrgb …`). Those lanes wire the dependency
in as part of Spec 81 layer 3; nothing consumes the crate yet.

## API

Anything that spawns a process or opens a socket is `async` on tokio, matching
the SDK client. Filesystem inspection and the pure builders are synchronous.

| Entry point | What it answers |
| --- | --- |
| `detect_binary() -> Option<OpenRgbBinary>` | PATH walk (`openrgb`/`OpenRGB`), platform install locations, portable AppImages (newest by version), then `flatpak info org.openrgb.OpenRGB`. The filesystem walk runs on `spawn_blocking`; the version comes from `--version` behind a 3 s timeout (`1.0rc3` from the `0.9+ (1.0rc3)` banner). |
| `probe_server(addr, timeout) -> ServerProbe` | SDK handshake plus controller count. Never fails; unreachable servers come back with `reachable: false` and the error text. |
| `install_hints() -> Vec<InstallHint>` | Detects pacman/apt/dnf/zypper/flatpak on PATH and returns exact commands in preference order. `install_hints_for(platform, &[InstallMethod])` is the pure selector. |
| `permission_checks() -> Vec<PermissionCheck>` | Linux only: udev rules present, `i2c-dev` loaded, `/dev/i2c-*` writable, and `/dev/hidraw*` writable for the VID:PID pairs the installed rules file covers (other HID nodes are informational). Each failure carries the remedy command. `linux_permission_checks_at(root)` runs against an injectable root. |
| `managed_config_dir(base_data_dir) -> ManagedConfigDir` | `<data>/openrgb`, the directory passed to OpenRGB's `--config`. |
| `partition_driver_ids(drivers, devices, known_driver_ids) -> DetectorPartitionPlan` | Which native drivers are withheld from OpenRGB (enabled, owning an enabled device), which families go back, and the USB devices behind them. `DriverFacts::from(&DriverSummary)` carries each driver's USB catalog from its published protocols. |
| `DetectorPartitionPlan::detector_rules() -> DetectorRules` | Translates the plan into per-detector rules: id-gated prefixes for drivers that published a USB catalog, whole-prefix rules for the rest. |
| `write_detector_partition(dir, rules, known_detectors) -> Result<DetectorPartition>` | Rewrites `Detectors.detectors` in `OpenRGB.json` by `rules`, preserving every other key, with a durable replace. |
| `detector_prefixes_for_drivers(driver_ids) -> Vec<String>` | Prefix lookup backed by the embedded `crates/hypercolor-openrgb-host/data/detectors.toml`. |
| `detector_usb_claim(name) -> Option<&DetectorUsbClaim>` | The USB devices (and vendor wildcards) an OpenRGB detector claims, from the embedded `crates/hypercolor-openrgb-host/data/detector_usb_ids.toml`. |
| `server_command(binary, dir, port) -> Result<ProcessSpec>` | `--server --server-host 127.0.0.1 --server-port <port> --noautoconnect --config <dir> --loglevel 4`; Flatpak wraps it in `flatpak run --filesystem=<dir> org.openrgb.OpenRGB`. Errors on non-UTF-8 paths and, for Flatpak, on paths containing `:`. |

### Types

- `OpenRgbBinary { path, kind: Native | Flatpak | AppImage, version }`. For
  Flatpak, `path` is the `flatpak` launcher and the app id travels in the
  process arguments.
- `ServerProbe { reachable, protocol_version, controller_count, error }`.
- `InstallHint { platform, method, command, note }` with `Platform` and
  `InstallMethod` enums.
- `PermissionCheck { id, ok, detail, remedy }`. Ids are the `CHECK_*`
  constants. `parse_rules_device_ids` and `parse_hid_id` expose the rules
  file and sysfs parsing behind the hidraw check.
- `ProcessSpec { program, args, env, cwd }`.
- `ManagedConfigDir { root }` with `config_path()`.
- `DetectorFamily { driver_id, prefixes, detectors }` and
  `DetectorPartition { disabled, enabled }`.
- `DetectorRules { disabled_prefixes, id_gated_prefixes, re_enable_prefixes,
  claimed_usb_ids, native_usb_ids }`; `DetectorRules::by_prefix` builds the
  conservative prefix-only form.
- `UsbDeviceId { vendor_id, product_id }`, serialized as `"vvvv:pppp"`, and
  `DetectorUsbClaim { devices, vendors }`.

## Detector partition semantics

A native driver is withheld when it is enabled and owns at least one enabled
device. Its USB catalog (the `vendor_id`/`product_id` pairs of its published
protocols) becomes `claimed_usb_ids`; every native driver's catalog, withheld
or not, becomes `native_usb_ids`.

The universe of detector names is the union of the existing file's map, the
caller's `known_detectors`, the embedded family seed lists, and every mapped
detector that claims a withheld device. For each name, in order:

1. a detector the id map ties to a device in `claimed_usb_ids` writes
   `false`, whatever its name;
2. a match on a whole-prefix family (a withheld driver that published no USB
   catalog) writes `false`;
3. a match on an id-gated family (a withheld driver with a catalog) writes
   `true` when the id map knows the name, since step 1 proved its devices are
   not natively claimable, and `false` when it does not (SMBus detectors,
   names from another OpenRGB release);
4. a match on a re-enable prefix writes `true`, which is how a caller hands a
   family back to OpenRGB once Hypercolor stops claiming it;
5. a detector the id map ties to a device in `native_usb_ids` writes `true`,
   releasing a detector step 1 disabled outside every family prefix;
6. otherwise the existing value is preserved, defaulting to `true` for names
   the file has never seen.

Prefix matching ignores ASCII case; id-map lookups are exact, because OpenRGB
keys its detector map by exact name. So with the native Razer driver owning a
Base Station V2, `Razer Base Station V2 Chroma` is disabled while
`Razer Kraken Ultimate` (no native protocol) stays with OpenRGB, and an
unknown `Razer ...` name stays disabled.

Nothing flips an existing `false` to `true` unless one of those rules hands
the name back, so a user's own toggles for unrelated detectors (Gigabyte,
MSI, ASRock) survive every rewrite. Invalid JSON or a non-object `Detectors`
section is an error, never clobbered.

## Launching the server

`server_command` always binds `127.0.0.1`: the SDK has no authentication and
OpenRGB 1.0rc3 still defaults to `0.0.0.0` (master flips the default, and we
pass the host explicitly either way). For Flatpak the supervisor must write
the partition first: `flatpak run --filesystem=<dir>` silently ignores a
directory that does not exist yet, and the path may not contain `:` because
`--filesystem=` treats it as an option separator.

## Data

`crates/hypercolor-openrgb-host/data/detectors.toml` maps each native driver id to the OpenRGB
detector prefixes it owns plus a seed list of detector names verified against
the map OpenRGB 1.0rc3 writes. It is embedded with `include_str!`, so shipping
binaries carry no data file. Add a `[[family]]` entry when a native driver
gains OpenRGB overlap, and check new names against a real `OpenRGB.json`.

`crates/hypercolor-openrgb-host/data/detector_usb_ids.toml` maps 1100
OpenRGB 1.0 detector names to the USB ids each claims. It is generated, never
hand-edited, from the udev rules an unmodified OpenRGB binary prints, which
keeps it on the black-box side of the Spec 68 provenance gate:

```bash
openrgb --print-udev-rules > /tmp/60-openrgb.rules
just openrgb-detector-ids /tmp/60-openrgb.rules
```

Regenerate it when OpenRGB ships a release; the `[source]` table records the
version and the input's sha256.

## Platform notes

- **Linux:** distro packages install `/usr/lib/udev/rules.d/60-openrgb.rules`;
  AppImage, Flatpak, and self-built binaries need the rules file installed by
  hand from the release tag (`UDEV_RULES_URL`). Released OpenRGB has no
  `--generate-udev-rules`; that flag exists only on master. SMBus needs
  `i2c-dev` plus `i2c-i801` (Intel) or `i2c-piix4` (AMD).
- **Windows:** HID works unelevated. SMBus needs PawnIO (OpenRGB 1.0rc2+
  dropped WinRing0) and Administrator or a service install. Hypercolor's
  native Windows SMBus path already uses PawnIO.
- **macOS:** HID only, no SMBus.
