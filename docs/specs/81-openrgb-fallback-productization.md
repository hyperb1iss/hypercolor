# Spec 81: OpenRGB Fallback Productization

## Status

Draft, implementation in flight (2026-09-07). Extends Spec 68 (bridge driver
slice) with the deferred supervisor milestone, the daemon primitives the
fallback needs to be safe without operator knowledge, and the user-facing
coverage flow. Distribution of OpenRGB binaries stays deferred behind the
legal gate in Spec 68.

## Goal

Anyone, on Linux, Windows, or macOS, can set up a rig where:

1. Hypercolor drives every device it supports natively.
2. The OpenRGB bridge drives what native drivers cannot, and only that.
3. OpenRGB is installed and configured under guidance only when that gap is
   real, never as a prerequisite.
4. Hardware neither stack covers turns into a device-support request with
   the facts already filled in.

The live baseline is the 2026-09-07 run on the O11D EVO RGB rig: Nollie 32,
two ENE DRAM sticks, and an ASUS Z790 driven through OpenRGB 1.0rc3 with the
TL wireless fans, Corsair pump, and Prism S native. Every gap below was hit
on that run.

## Non-Goals

- Replacing native drivers with OpenRGB where a native driver exists.
- Bundling or auto-installing OpenRGB binaries (Spec 68 distribution lane).
- Linking to OpenRGB code or reading its sources (Spec 68 provenance gate).
- Emitting `SAVEMODE`. It writes device NVRAM and stays banned forever.

## Policy: native first

Ownership is decided per physical device, not per driver:

| device state | native driver | bridge | result |
|---|---|---|---|
| native protocol exists, driver enabled, device enabled | drives | must not see it | native |
| native protocol exists, device disabled by user | released | may drive | bridge, user chose |
| native protocol exists, driver disabled | not registered | may drive | bridge, user chose |
| no native protocol | none | drives | bridge |
| both stacks active on the same silicon | drives | output-disabled with reason | conflict guard, native wins |

"Must not see it" is enforced on the OpenRGB side by the detector partition
the supervisor writes into a Hypercolor-managed OpenRGB config directory, and
on the Hypercolor side by the conflict guard. Neither alone is sufficient:
the partition is a name-based approximation, the guard is exact but only
protects after both stacks have already opened the hardware once.

## Layer 1: Bridge driver hardening

Crates: `hypercolor-driver-openrgb`, `hypercolor-openrgb-sdk`. Spec 68 is
amended where noted.

### 1.1 Write-path shape guard

`write_controller_colors` must compare the frame length with the route's
`capabilities.led_count`. On mismatch: pad or truncate to the controller's
count so OpenRGB does not silently discard the packet, log once per route,
and surface `disabled_reason = "zone shape changed (was N, now M); rescan"`
when the mismatch came from the controller side. A zero-LED controller is a
hard output-disable, never a stream of accepted no-ops.

### 1.2 Shape changes reach the daemon

`refresh_connected_route` currently replaces `route.info` inside the writer
task with no notification. After any re-enumeration whose `led_count` or
segment list differs, the driver must request a device reconnect through the
`DriverHost` lifecycle so the registry republishes `DeviceInfo` and the
layout converges. Re-enumeration moves off the frame path onto a
per-endpoint task.

### 1.3 Brightness

The selected writable mode is written with `brightness = brightness_max`
when the mode advertises a brightness range, instead of echoing whatever the
server reported. `verify_controller_output_mode` checks brightness too. A
dark controller with correct mode and correct frames is the failure this
prevents.

### 1.4 Cadence

Add a detector-class default table for `target_fps`: `smbus` matches the
native SMBus backend cadence, `hid` and everything else keep 30. The writer
task paces to `target_fps` with latest-value drop, so a slow controller
never runs the bus faster than its class allows. The global render tier is
untouched.

### 1.5 Connection model

One SDK connection per endpoint, shared by every controller on it, guarded
by a mutex. `DEVICE_LIST_UPDATED` is drained by the endpoint task, which
re-enumerates once and updates every route. Discovery reuses the endpoint
connection when one is open. Before dropping a connection, drain pending
packets and `shutdown(Write)` so OpenRGB sees a clean close. Reconnect uses
bounded backoff (1 s doubling to 60 s with jitter) inside the driver; the
daemon lifecycle's reconnect remains the outer loop.

### 1.6 Identity surfaced

Discovery metadata gains `location` (verbatim OpenRGB string) and `serial`.
`DeviceInfo` for a bridge device carries the endpoint in its connection
label. The daemon consumes `identity_confidence`, `detector_class`,
`output_enabled`, and `disabled_reason` (Layer 2.5).

### 1.7 Zone sizes (Spec 68 amendment)

`RESIZEZONE` moves from forbidden to gated. `OpenRgbClientConfig` gains
`allow_zone_resize: bool` (default false); `encode_client_packet` refuses
the opcode unless the flag is set, and the existing refusal test keeps
covering the default. `OpenRgbClient::resize_zone(controller, zone, size)`
clamps to `leds_min..=leds_max` and rejects `ZoneType::Single`.

Driver config gains `zone_sizes: BTreeMap<String, BTreeMap<String, u32>>`
keyed by controller fingerprint, then zone name. On connect, when a zone's
`leds_count` differs from the configured size, the driver resizes, waits
for the server's re-enumeration, and republishes the shape (1.2). Sizes are
user-approved data: they enter config through the normal config API, the
CLI, or the wizard, never inferred. OpenRGB profiles do not persist SDK-side
resizes (verified 2026-09-07), so this is the only durable path.

`SAVEMODE` stays forbidden with no flag.

### 1.8 Temporary identify

`OpenRgbBackend` implements `supports_temporary_direct_control` as
`supports_direct && total_led_count() > 0`. Identify on a `Known` bridge
device adopts, connects, flashes, and tears down (restoring the previous
mode). An output-disabled route returns its `disabled_reason` instead of
"not connected".

### 1.9 Tests

Everything Spec 68 claims and the survey found missing: `DEVICE_LIST_UPDATED`
remap, reconnect sequence, per-controller `target_fps`, slow-controller
isolation, `configure_controller_output` sequence, brightness write and
verify, all four teardown policies, shape-guard pad/truncate/disable, zone
resize gate and clamp, discovery over a fake server. The stale
`socket = "/run/openrgb.sock"` fixtures in `hypercolor-core` and
`hypercolor-types` config tests are replaced with the real schema.

## Layer 2: Daemon primitives

Crates: `hypercolor-daemon`, `hypercolor-core` (USB scanner and hotplug),
`hypercolor-types`, `hypercolor-cli` (devices subcommands only).

### 2.1 Unclaimed hardware inventory

`usb_scanner.rs` and `usb_hotplug.rs` drop devices with no protocol match
silently. They now record them in an `UnclaimedDeviceStore` shared the same
way `UsbProtocolConfigStore` is: vendor id, product id, manufacturer and
product strings, serial, bus path, interface classes (nusb), and
`claimable_by: Option<driver_id>` when a descriptor exists but its driver is
disabled. The store is a snapshot replaced per scan and patched per hotplug
event.

API: `GET /api/v1/devices/unclaimed` answers `ListResponse` of
`UnclaimedDevice`. Event: `HypercolorEvent::UnclaimedDevicesChanged { count }`
on the default `events` topic. CLI: `hypercolor devices unclaimed`. SMBus has
no enumerate-then-filter step today; unclaimed SMBus is out of scope here.

### 2.2 Coverage and conflict guard

`GET /api/v1/devices/coverage` joins three sources per physical device:
native registry devices (serial, USB path, SMBus bus and address from
discovery metadata), bridge routes (OpenRGB `serial` and parsed `location`),
and the unclaimed store. Match keys, in order: serial (case-insensitive,
trimmed), SMBus bus plus address, USB path. Each row reports
`{ identity, native: Option<{device_id, driver_id, state}>,
bridge: Option<{device_id, output_enabled, disabled_reason}>,
unclaimed: bool, active: native | bridge | none | conflict }`.

The guard runs after every discovery pass and after any bridge device
connects: when a row has a renderable native device and a bridge route that
is output-enabled, the bridge route is output-disabled with
`disabled_reason = "native driver owns this device (<driver_id>)"` and a
`DeviceStateChanged` event is published. Native never yields automatically;
the user hands a device to the bridge by disabling it natively
(`PUT /devices/{id}` with `enabled: false`), which already releases the HID
or SMBus handle.

Discovery of the bridge is quiesced while a native SMBus scan runs and vice
versa (one `Mutex` in the discovery worker); the 2026-09-07 run produced
garbage DRAM LED counts from concurrent probes.

### 2.3 Live driver registration

`register_enabled_device_backends` runs only at startup, so enabling the
bridge in config today gives discovery but no output until restart while
`requires_restart` reports false. A `ConfigChanged` reconciler registers or
unregisters the driver's output backend when `drivers.<id>.enabled` flips.
Disabling unregisters after disconnecting that backend's devices.

### 2.4 Diagnose check

`"openrgb"` joins `DEFAULT_SAFE_CHECKS`: cfg-free TCP connect to each
configured endpoint plus a protocol-version handshake through the SDK
crate, reporting reachable, negotiated version, controller count, and the
count of output-disabled routes with their reasons. The CLI and MCP
`diagnose` pick it up unchanged. The check must not add an eleventh
`cfg(target_os)` file to the daemon.

### 2.5 Device summary

`DeviceSummary` gains `bridge: Option<BridgeDeviceSummary { endpoint,
controller_index, identity_confidence, detector_class, output_enabled,
disabled_reason, protocol_version }>` filled from discovery metadata for
`transport == bridge`. `device_connection_summary` labels bridge devices
with their endpoint. The `UpdateDeviceRequest` doc comment says PUT.

## Layer 3: Guided install and supervision

Crates: new `hypercolor-openrgb-host` (platform crate pattern, stubs on
every target), `hypercolor-app`, `hypercolor-cli` (new `openrgb` verb),
daemon MCP prompt and tool, docs.

### 3.1 `hypercolor-openrgb-host`

Neutral types on every target; OS calls behind `cfg(target_os)` inside the
crate only.

- `detect_binary() -> Option<OpenRgbBinary { path, kind: Native | Flatpak |
  AppImage, version }>`: PATH walk plus known locations (`/usr/bin/openrgb`,
  `flatpak info org.openrgb.OpenRGB`, `%ProgramFiles%\OpenRGB\OpenRGB.exe`,
  `/Applications/OpenRGB.app/Contents/MacOS/OpenRGB`).
- `probe_server(addr) -> ServerProbe { reachable, protocol_version,
  controller_count }` via the SDK crate.
- `install_hints() -> Vec<InstallHint { platform, method, command, note }>`
  chosen by detected package manager: pacman, apt, dnf, zypper, Flatpak
  (`flatpak install flathub org.openrgb.OpenRGB`), winget
  (`winget install -e --id OpenRGB.OpenRGB`), macOS direct download (Intel
  and Apple Silicon zips). Windows notes: SMBus needs PawnIO and
  administrator; OpenRGB 1.0rc2+ no longer ships WinRing0. macOS note: HID
  only, no SMBus.
- `permission_checks()` on Linux: `60-openrgb.rules` present, `i2c-dev`
  loaded, `/dev/i2c-*` and `/dev/hidraw*` writable by the current user; each
  failing check carries the exact remedy command from OpenRGB's own docs.
- `managed_config_dir()` under Hypercolor's data dir, and
  `write_detector_partition(dir, disabled_detectors)` producing an
  `OpenRGB.json` whose `Detectors.detectors` map disables the given names.
  The detector-name map lives in `data/openrgb/detectors.toml`: for each
  native driver family, the OpenRGB detector name prefixes it owns
  (`Razer `, `Lian Li `, `Corsair `, `Dygma `, `Nollie `, `ASUS Aura`,
  `ENE SMBus DRAM`). The partition disables prefixes for every native driver
  that is enabled and has at least one enabled device.
- `server_command(binary, dir) -> ProcessSpec` =
  `--server --server-host 127.0.0.1 --noautoconnect --config <dir>`, plus
  `--loglevel 4`. Never `0.0.0.0`: the SDK has no authentication.

### 3.2 App supervisor

`supervisor/plan.rs` gains `OpenRgbPlan { Adopt(existing), Spawn(spec),
Hold(reason: NotInstalled(hints) | PermissionsMissing(checks) |
BridgeDisabled) }` as a pure function of `(binary, server probe,
permission checks, bridge config)`. Execution reuses the daemon child
machinery (platform command configuration, job object or parent-death
guard, kill-on-drop). No watchdog restarts: an OpenRGB exit is user intent
until the user asks again. Tauri commands: `detect_openrgb`, `start_openrgb`,
`stop_openrgb`, `openrgb_install_hints`. OpenRGB joins the known RGB
conflict table on Windows so the existing conflict UI names it.

### 3.3 CLI

`hypercolor openrgb status | hints | partition | start | stop | resize`.
`status` prints binary, server probe, bridge config, and coverage rows that
involve the bridge. `partition` writes the managed detector config from the
daemon's coverage view. `resize <device> <zone> <size>` writes
`drivers.openrgb.zone_sizes` through the config API and triggers reconnect.
The CLI sends a single-zone object merge to
`PATCH /config/keys/drivers.openrgb.zone_sizes`; the config manager merges
under its existing write lock so concurrent clients preserve sibling zones.
`start` and `stop` drive the app supervisor when it is running and fall
back to spawning the process directly when it is not.

### 3.4 MCP

Prompt `openrgb_setup` (not `setup_rig`, which Spec 70 reserves) walks an
agent through: coverage, install hints, partition, enable bridge, discover,
zone sizes, layout. Tool `openrgb_status` returns the same payload as the
CLI `status` for agents that cannot shell out.

The shared status payload is also available at
`GET /api/v1/system/openrgb`. The CLI, MCP tool, and browser use the same
typed response for host platform, installation, endpoint probes, permission
checks, install hints, coverage, and output-disabled count. Probes run even
when bridge output is disabled so setup can inspect readiness first.

### 3.5 Docs

`hardware/openrgb-fallback.md` gets a per-platform install matrix, the
PawnIO and macOS notes, the managed config directory, zone sizes, and the
coverage view. `guide/your-first-10-minutes.md` and `guide/finding-devices.md`
stop saying "stop OpenRGB first" and point at the bridge flow.
`troubleshooting/_index.md` adds "OpenRGB is installed but Hypercolor cannot
reach it". Spec 68's status and supervisor sections point here.

## Layer 4: UI

Crate: `hypercolor-ui`.

- Device cards and the detail drawer show a bridge badge when
  `presentation.icon == "bridge"` (first consumer of that field) and the
  `disabled_reason` from `DeviceSummary.bridge` when output is disabled.
- Devices page gains an "Unclaimed hardware" section fed by
  `GET /devices/unclaimed` and refetched on `unclaimed_devices_changed`.
  Each row offers "Request support" (prefilled issue-form URL with vendor,
  model, VID:PID, platform, existing-support text) and, when the bridge is
  reachable and lists a controller with a matching serial, "Available via
  OpenRGB".
- Settings → Discovery gains an OpenRGB card: reachable, version,
  controllers, output-disabled count, install hints when the binary is
  missing (from the app when running inside it, from diagnose otherwise),
  and the ownership mode selector. Hidden when the `openrgb` driver is not
  compiled in.
- Welcome overlay gains an OpenRGB step gated on unclaimed hardware.

## Layer 5: Skill

`skills/rig-setup` gains a coverage phase between inventory and interview:
`GET /devices/unclaimed` and `/devices/coverage`, the OpenRGB decision, the
guided install (`hypercolor openrgb hints`), partition, enable, discover,
zone sizes, then layout. Bridged strimers are raw rows until multi-zone slot
groups exist (tracked separately). `scripts/request_support.py` builds the
prefilled issue URL or files with `gh` when authenticated.

## Verification Gates

- `just verify` on every lane; `just ui-test` for Layer 4.
- Fake-server tests listed in 1.9.
- Live: the O11D rig with the bridge owning Nollie, ENE DRAM, and Z790;
  DRAM lit; server restart preserves zone sizes through the driver; killing
  the native `nollie` device while the bridge has a Nollie route triggers
  the conflict guard and re-enabling native disables the bridge route.
- Windows: `hypercolor openrgb hints` names winget and PawnIO; a portable
  OpenRGB started by the supervisor binds loopback; a Razer device driven
  natively is refused by the bridge.
- macOS: hints name the Apple Silicon zip; the bridge lists HID controllers
  only.
- Independent verification agent before any lane reports done.

## Open Questions

- Whether a manual `POST /devices/{id}/connect` is worth adding for the
  wizard, or identify's temporary connect covers every flash-before-layout
  need.
- Multi-zone slot groups (a strimer template across six 20-LED bridge
  zones) live in the attachment layer and are tracked as their own spec
  item.
