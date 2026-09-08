# Coverage: native first, bridge for the gap, request the rest

Coverage answers one question per physical device: who drives it. The answer decides
whether OpenRGB enters the setup at all. Hypercolor drives everything it has a driver
for; the OpenRGB bridge drives only what native cannot; OpenRGB gets installed only when
an unclaimed device needs it, never as a prerequisite; and hardware neither stack covers
becomes a device-support request with the facts already filled in.

## Reading the report

`python3 scripts/coverage.py` prints four groups. It asks `GET /devices/coverage` first
and rebuilds the same picture from `GET /devices`, `GET /devices/unclaimed`, and the
host's USB inventory diffed against `GET /drivers` when the daemon predates those routes
(they answer 404 with the code `device_not_found`, because the path falls into
`/devices/{id}`). `hypercolor devices coverage` and `hypercolor devices unclaimed` print
the daemon's own view; the MCP tool `openrgb_status` carries the bridge rows.

| group | `active` | meaning | you do |
|---|---|---|---|
| native | `native` | a Hypercolor driver owns the device | nothing |
| bridge | `bridge` | the OpenRGB bridge owns it, no native driver can | place it like any device |
| unclaimed | `none`, `unclaimed: true` | enumerated on the bus, nobody drives it | the decision table below |
| conflict | `conflict` | native and bridge both reached the same silicon | leave it, or hand it over deliberately |
| known, not driving | `none` | a native device that is known but not connected (off, unpaired, disabled) | Phase 0 work: power, pairing, `enabled` |

One physical device can carry two facts. The Nollie 32 driven by the bridge also shows
up natively unclaimed with `claimable_by: nollie` when the native driver is disabled;
the report joins them by serial into one row so you read "bridge drives it, native could".

## The decision table for an unclaimed device

Work down; stop at the first row that applies.

| if | then | why |
|---|---|---|
| `claimable_by` names a driver | `hypercolor config set drivers.<id>.enabled true` (or `PUT /config/keys/drivers.<id>.enabled` with body `true`), then `hypercolor devices discover` | a native driver exists; it is only switched off. Native output, no OpenRGB |
| OpenRGB lists the device (its supported-devices page, or the owner already uses it) | the guided install ladder below | the bridge fills exactly this gap |
| neither | `python3 scripts/request_support.py --vid-pid VVVV:PPPP` | the maintainers need the VID:PID, platform, and what drives it today; the script fills those and dedupes against open issues |

Tell the owner which row each device landed in. A layout that covers what is drivable
today is a finished layout; the request is the path for the rest.

Some devices the host scan lists are not lighting at all (a flash drive, a USB audio
interface, the board's Bluetooth radio). The fallback hides a device when its interfaces,
HID set aside, are all of classes that rule out lighting (audio, storage, video,
wireless, application-specific); a pure HID device or a vendor-specific one stays
visible because RGB controllers live there. `--all` shows everything. When in doubt, the
owner knows what the thing is.

The fallback also has a fifth group, **known to a native driver, not adopted**: the
VID:PID is in an enabled driver's protocol table, yet the daemon has no device for it.
That is a permissions story (udev rules, hidraw access) or another program holding the
handle, never a coverage decision; fix the access and rescan before reading further.

## Conflict rows

Ownership is per physical device, not per driver. When both stacks reach the same
silicon, native keeps driving and the bridge route is output-disabled with
`disabled_reason = "native driver owns this device (<driver_id>)"`. The guard runs after
every discovery pass and whenever a bridge device connects. Native never yields on its
own: the owner hands a device to the bridge by disabling it natively (`PUT /devices/{id}`
with `{"enabled": false}`, which releases the HID or SMBus handle), after which the next
discovery pass re-enables the bridge route. Read `disabled_reason` before touching
anything; it names the owner. An empty reason with output disabled means the bridge
itself parked the route (duplicate fingerprint, ownership mode), and `hypercolor openrgb
status` says which.

## The guided install ladder

Each rung has a check. Do not climb past a failing check.

1. **Hints.** `hypercolor openrgb hints` prints the install command for the detected
   package manager plus the platform notes. Use it rather than the table below whenever
   the verb exists; the table is for daemons that predate it.
2. **Install.** Run the hint. Check: `openrgb --version` prints 1.0rc2 or later.
3. **Permissions.** Linux needs the `60-openrgb.rules` udev file and, for DRAM and
   motherboards, `i2c-dev` plus the chipset driver (`i2c-i801` Intel, `i2c-piix4` AMD).
   Windows needs PawnIO and an administrator prompt for SMBus, nothing extra for HID.
   macOS is HID only. `hypercolor openrgb hints` and `hypercolor diagnose` report the
   failing check with its remedy. Check: the user can open `/dev/hidraw*` and, if SMBus
   is involved, `/dev/i2c-*`.
4. **Partition.** `hypercolor openrgb partition` writes an OpenRGB config directory under
   Hypercolor's data dir (`~/.local/share/hypercolor/openrgb` on Linux) that disables the
   OpenRGB detectors for every native driver that is enabled and has an enabled device.
   The owner's own `~/.config/OpenRGB` is never edited. Check: the directory holds an
   `OpenRGB.json`.
5. **Enable the bridge.** `hypercolor config set drivers.openrgb.enabled true`, then the
   ownership mode: `drivers.openrgb.ownership.mode` is `detector_partitioned` for the
   normal "bridge drives the gap" setup, `open_rgb_owned` when the owner wants OpenRGB to
   own everything it can see (still guarded by the conflict rule), `disabled` to park it.
   Check: `hypercolor diagnose` shows the `openrgb` check.
6. **Start the server.** `hypercolor openrgb start` runs
   `openrgb --server --server-host 127.0.0.1 --server-port 6742 --noautoconnect --config <dir> --loglevel 4`
   (the desktop app supervises it when running). Loopback only: the SDK has no
   authentication. Check: `hypercolor diagnose` reports the endpoint reachable with a
   protocol version and a controller count.
7. **Discover.** `hypercolor devices discover --target openrgb`. Check: the bridged
   devices appear in `hypercolor devices list` with `openrgb:` layout ids.
8. **Size hub zones.** Hubs such as the Nollie 32 arrive as 32 zones of 0 LEDs
   ("Channel 1..16", "Channel ATX 1..6", "Channel GPU 1..6", "Channel EXT 1..4"). Size the
   ones with hardware: `hypercolor openrgb resize <device> "<zone>" <leds>` writes
   `drivers.openrgb.zone_sizes` and reconnects. Record the same sizes in the rig spec's
   `bridge.zone_sizes` so `gen_layout.py apply` restores them after a reinstall. Check:
   `GET /devices/{id}` shows the LED counts you set.
9. **Identify.** Flash one bridged zone with `POST /devices/{id}/attachments/{slot}/identify`
   (bridged devices accept identify through a temporary connect even while `known`).
   Light proves the whole chain. Then continue with Phase 2.

### Fallback install table

For daemons without `hypercolor openrgb hints`. Prefer the hints when they exist.

| platform | install | notes |
|---|---|---|
| Arch and derivatives | `sudo pacman -S openrgb` | udev rules ship with the package |
| Flatpak (any distro) | `flatpak install flathub org.openrgb.OpenRGB` | run as `flatpak run org.openrgb.OpenRGB`; udev rules still need installing on the host |
| Debian, Ubuntu, Fedora, openSUSE | `.deb` / `.rpm` from https://openrgb.org/releases.html | install `60-openrgb.rules` to `/etc/udev/rules.d/`, `sudo udevadm control --reload-rules && sudo udevadm trigger` |
| Any Linux | AppImage from https://openrgb.org/releases.html | same udev step |
| Windows | `winget install -e --id OpenRGB.OpenRGB` | SMBus needs PawnIO and an administrator session; 1.0rc2+ no longer ships WinRing0 |
| macOS | Intel or Apple Silicon zip from https://openrgb.org/releases.html | HID devices only, no SMBus |

## OpenRGB facts that bite

- **Never run OpenRGB detection while native SMBus drivers are live.** Two probes on one
  bus return garbage: ENE DRAM sticks read 35 and 3 LEDs instead of 8 and 8. Disable the
  native SMBus driver (or let the partition do it) before the first OpenRGB scan, and let
  the daemon serialize the two (it quiesces bridge discovery during a native SMBus scan).
- **Zone sizes do not survive an OpenRGB restart.** `openrgb --save-profile` stores colours
  and modes, not sizes. Only `drivers.openrgb.zone_sizes` restores them, which is why the
  rig spec carries them.
- **The `openrgb` CLI exits nonzero on success** for `-d N -z Z -sz S` against a running
  server. Check the result (`openrgb --list-devices`, or `GET /devices/{id}`) rather than
  the exit status.
- **`openrgb --list-devices` without a server** prints "connection attempt failed" and then
  detects locally. Benign, and `--noautoconnect` skips it. Local detection is a full probe,
  so the SMBus rule above applies to that command too.
- **Frames can succeed into a dark controller.** A zero-LED zone accepts frames ("sent,
  0 failed") and shows nothing. Discriminators, in order: `hypercolor devices coverage`
  (is the route output-enabled), the device's `bridge.disabled_reason`, LED counts in
  `openrgb --list-devices` or `GET /devices/{id}`, then an identify flash.
- **Zero-LED zones can share a slot id.** Until they are sized, a hub's empty zones may
  collapse onto one alias (`channel-16` for Channel 7..16, `channel-ext-4` for the empty
  GPU and EXT zones). Size before binding, and address zones by segment name in raw
  zones and `bridge_rows`, which never go through the slot id.
- **A GPU OpenRGB does not detect** (unlisted PCI subsystem id, such as the ASUS
  `1043:8970` RTX 4070 SUPER) is an OpenRGB issue: file it upstream with OpenRGB, not with
  Hypercolor. Hypercolor's request form is for devices OpenRGB does not cover either.
- **Fan templates on bridge slots.** Bridged strip slots accept fan and ring templates on
  current daemons. If `PUT /devices/{id}/attachments` answers
  `template 'lian-li-sl-infinity-fan' is not allowed for slot 'channel-1'`, the daemon
  predates that change: clone the fan template as a user template with category `strip`
  (`POST /attachments/templates` with the same topology) and bind that. Treat it as a
  version note, not the default path.

## What `known` means for a bridged device

Native devices at status `known` are not connected and cannot be placed. Bridged devices
are different by design: they sit at `known` until the active layout targets them, then
connect. Identify works on them through a temporary connect, so you can flash a bridged
zone before any layout exists. Do not "fix" a `known` bridged device; place it and apply.

## Reading `disabled_reason`

| reason | source | you do |
|---|---|---|
| `native driver owns this device (<driver_id>)` | conflict guard | nothing, or hand over by disabling native |
| duplicate fingerprint text | bridge (two controllers with the same serial or location) | `hypercolor openrgb status`, then pick one with the ownership mode |
| ownership or detector-class text | bridge partition (`detector_partitioned` mode and a native-claimed class) | expected; the native driver has it |
| `null` with `output_enabled: false` | the owner disabled the device | `PUT /devices/{id}` with `enabled: true` when wanted |

## Bridged devices in the layout

- **Hubs** (Nollie, Uni Hub) expose one generic slot per zone (`channel-1`, `channel-atx-1`,
  ...) with `led_start` cumulative across the device. Fan templates bind to them like
  native slots; keep `led_start_override: 0` on nothing, the daemon's suggested windows
  are right once zones are sized.
- **Strimers** through a hub are one zone per row (six 20-LED "Channel ATX n", four or six
  27-LED "Channel GPU n"). The 120- and 108-LED strimer templates cannot bind to a 20-LED
  zone, so they become a `bridge_rows` block in the rig spec: one raw strip per row,
  stacked at a pitch from the header anchor. `references/rig-spec.md` has the block and
  `references/rigs/o11d-evo-rgb-reversed-bridge-example.json` the worked example.
- **DRAM and motherboards** are raw zones from their segment (`DRAM`, `Aura Mainboard`),
  exactly like their native counterparts with the `openrgb:` layout id.
- **Layout id versus fingerprint.** The layout id is
  `openrgb:127-0-0-1:6742:serial:0994fa72ab3cae43` (lower-cased, dots replaced) and is
  what layouts and rig specs use. The fingerprint is a different string,
  `bridge:openrgb:127.0.0.1:6742:serial:0994FA72AB3CAE43` (namespace:driver:endpoint:
  identity, serial in OpenRGB's own case, `location:<path>` when there is no serial), and
  is what `zone_sizes` and `controller_fps` are keyed by. `GET /devices/{id}` carries it as
  `bridge.fingerprint` and `hypercolor devices info <id>` prints it on daemons that ship
  the coverage routes; the bridge matches the key case-insensitively.
- **Spotting bridge devices in scripts.** Key on `origin.driver_id == "openrgb"` (or the
  `openrgb:` layout-id prefix), never on `origin.transport == "bridge"` alone: the ROLI
  driver reports transport `bridge` too for its BLE blocks. `scripts/coverage.py` does this.
