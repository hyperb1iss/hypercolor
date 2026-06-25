+++
title = "OpenRGB fallback"
description = "The OpenRGB SDK bridge: a user-run server on :6742, ownership modes, detector partition, and when to reach for it."
weight = 90
+++

# OpenRGB fallback

Hypercolor ships a bridge driver that talks to a running [OpenRGB](https://openrgb.org)
server over the OpenRGB SDK wire protocol on TCP port 6742. The bridge lets you control
hardware that OpenRGB already supports but Hypercolor does not yet have a native driver
for, without writing any code.

{% callout(type="warning") %}
The OpenRGB driver ships compiled in but is **disabled in config by default**. You must
enable it and choose an ownership mode before any devices appear. OpenRGB is not bundled
or managed by Hypercolor — you install, configure, and run it separately.
{% end %}

---

## When to use the bridge

Reach for the bridge when a device shows up in OpenRGB but not in Hypercolor's native
discovery, or when you already have OpenRGB configured and want Hypercolor to drive it
for synchronized effects.

Check the [hardware compatibility matrix](@/hardware/compatibility.md) first. If a
native Hypercolor driver covers your hardware, use it — native drivers connect directly,
have lower latency, and carry stable device identities without depending on a second
process.

---

## Prerequisites

1. Install OpenRGB from [openrgb.org](https://openrgb.org) or your distribution's
   package repository.
2. Complete OpenRGB's own Linux setup — udev rules, i2c-dev module for SMBus hardware,
   and any chipset-specific kernel parameters. Follow the
   [OpenRGB udev rules guide](https://openrgb.org/udev.html)
   for that; it is separate from Hypercolor's udev rules.
3. Start the OpenRGB SDK server before triggering Hypercolor discovery:

```bash
openrgb --server
```

Confirm it is listening:

```bash
ss -tlnp | grep 6742
```

The server binds to `127.0.0.1:6742` by default. Hypercolor connects on demand during
discovery and reconnects automatically with exponential backoff (250 ms initial, 5 s cap)
if the connection drops.

---

## Enabling the driver

Add or update the `openrgb` driver entry in your Hypercolor configuration:

```toml
[drivers.openrgb]
enabled = true
endpoints = ["127.0.0.1:6742"]
detector_partition_confirmed = false
default_target_fps = 30
teardown_policy = "restore_previous_or_leave"

[drivers.openrgb.ownership]
mode = "open_rgb_owned"
```

Restart the daemon, then run discovery:

```bash
hypercolor service restart
hypercolor devices discover --target openrgb
```

---

## Ownership modes

The bridge uses an ownership mode to decide which OpenRGB-reported controllers receive
Hypercolor output. The default mode is `disabled`; no devices surface until you set
one of the three modes.

### `disabled`

Discovery short-circuits: the bridge does not contact OpenRGB and surfaces no
devices. This is the default.

### `open_rgb_owned`

Every controller OpenRGB reports is eligible for Hypercolor output, subject to confidence
filtering. Use this when Hypercolor has no native drivers for your hardware, or when you
have deliberately disabled the conflicting native drivers.

```toml
[drivers.openrgb.ownership]
mode = "open_rgb_owned"
```

### `detector_partitioned`

Only controllers whose OpenRGB detector class appears in `allowed_detector_classes` are
eligible. Use this to run Hypercolor's native drivers alongside the bridge without
conflicts — route only the device types OpenRGB owns into Hypercolor output.

```toml
[drivers.openrgb]
enabled = true
detector_partition_confirmed = true

[drivers.openrgb.ownership]
mode = "detector_partitioned"
allowed_detector_classes = ["virtual"]
```

`detector_partition_confirmed = true` is required here. It is a deliberate safety gate:
set it only after you have confirmed that OpenRGB's plugin configuration matches the
partition — meaning OpenRGB's own i2c-smbus and HID detectors are disabled (or limited)
for the hardware Hypercolor's native drivers own. Forgetting this causes both processes to
write to the same controllers and corrupt device state.

---

## Detector classes

OpenRGB classifies each controller by the subsystem that detected it. Hypercolor maps
these to internal detector classes used in `allowed_detector_classes` and
`native_claimed_detector_classes`:

| Detector class | OpenRGB device types |
|---|---|
| `smbus` | Motherboard, DRAM, GPU |
| `hid` | Keyboard, mouse, cooler, strip, and most USB peripherals |
| `virtual` | Virtual / light device types |
| `unknown` | Unknown or other device type |

### Reserving classes for native drivers

Use `native_claimed_detector_classes` to exclude device classes from bridge output,
leaving them for Hypercolor's native HAL drivers:

```toml
[drivers.openrgb]
enabled = true
detector_partition_confirmed = true

[drivers.openrgb.ownership]
mode = "open_rgb_owned"
native_claimed_detector_classes = ["smbus", "hid"]
```

Controllers in the claimed classes are discovered but disabled for output — they appear
in the device list but are not writable through the bridge.

{% callout(type="warning") %}
Low-confidence `hid` and `smbus` controllers are always output-disabled, regardless of
`allow_low_confidence`. Index-based identity is not stable enough for contention-prone
devices. A controller falls to low confidence when OpenRGB reports it without a serial
number and without a location string, or without a usable vendor and name.
{% end %}

---

## Per-LED mode requirement

Hypercolor streams per-LED colors every render frame. A controller is ineligible for
output if none of its OpenRGB modes carry the per-LED color flag
(`MODE_FLAG_HAS_PER_LED_COLOR`). The bridge checks this on connect and on every
reconnect.

If a controller appears in discovery but is immediately disabled, confirm in OpenRGB's
UI that the controller has a mode with per-LED control enabled.

---

## Frame rate

The default output rate for all OpenRGB controllers is 30 FPS. Override it globally with
`default_target_fps`, or per-controller using `controller_fps`. The `controller_fps`
key is a map from fingerprint string or detector class name to FPS:

```toml
[drivers.openrgb]
enabled = true
default_target_fps = 30

[drivers.openrgb.controller_fps]
"hid" = 60
"bridge:openrgb:127.0.0.1:6742:serial:KB001" = 45
```

The fingerprint string for each controller is shown in `hypercolor devices info <id>`
under device metadata.

---

## Teardown policy

When Hypercolor disconnects from a controller, the teardown policy controls what the
device does next:

| Value | Behavior |
|---|---|
| `restore_previous_or_leave` | Restore the pre-connect mode if known; otherwise leave the last frame. This is the default. |
| `restore_previous_or_blackout` | Restore the pre-connect mode if known; otherwise write black. |
| `blackout` | Always write black before disconnecting. |
| `leave_last_frame` | Leave whatever frame was last sent. |

```toml
[drivers.openrgb]
teardown_policy = "restore_previous_or_leave"
```

---

## Identity and fingerprinting

Each OpenRGB controller gets a stable fingerprint so Hypercolor can re-identify it
across OpenRGB restarts and controller reorders. The fingerprint strategy depends on what
OpenRGB reports, in priority order:

1. Serial number — confidence **high**
2. Location string (e.g. `hidraw0`) — confidence **high**
3. Vendor + name + zone count + LED count shape — confidence **medium**
4. Controller index only — confidence **low** (output disabled for `hid` and `smbus`)

When two controllers produce the same fingerprint, for example identical devices with
no serial, both are discovered but disabled, and annotated with "collides with another
controller." Resolve this by assigning unique serial numbers or ensuring OpenRGB
enumerates only one.

---

## Multiple endpoints

You can bridge multiple OpenRGB instances. Fingerprints include the endpoint address, so
the same physical controller on two endpoints gets two distinct device IDs:

```toml
[drivers.openrgb]
enabled = true
endpoints = ["127.0.0.1:6742", "192.168.1.20:6742"]
allow_insecure_remote = true

[drivers.openrgb.ownership]
mode = "open_rgb_owned"
```

{% callout(type="danger") %}
The OpenRGB SDK protocol carries no authentication or encryption. Never expose port 6742
to untrusted networks without a firewall rule or VPN tunnel. Setting
`allow_insecure_remote = true` is required for any non-loopback endpoint and is an
explicit opt-in to the associated risk.
{% end %}

---

## Protocol version

Hypercolor negotiates the OpenRGB SDK protocol version on connect, supporting versions 1
through 5 (OpenRGB 1.0). Protocol version 5 is required for the `startup_rescan` option,
which asks OpenRGB to re-probe hardware on connection. The negotiated version for each
controller is shown in device metadata as `protocol_version`.

---

## Full configuration reference

```toml
[drivers.openrgb]
enabled = false                          # disabled by default
endpoints = ["127.0.0.1:6742"]          # OpenRGB SDK server addresses
allow_insecure_remote = false            # must be true for non-loopback endpoints
connect_timeout_ms = 750
read_timeout_ms = 750
write_timeout_ms = 750
startup_rescan = false                   # send rescan request on connect (protocol v5+)
auto_connect = true                      # auto-connect output-enabled controllers on discovery
detector_partition_confirmed = false     # required when using partitioned or native-claimed ownership
default_target_fps = 30
teardown_policy = "restore_previous_or_leave"

[drivers.openrgb.controller_fps]
# "hid" = 60
# "bridge:openrgb:127.0.0.1:6742:serial:SER123" = 45

[drivers.openrgb.ownership]
mode = "disabled"                        # disabled | detector_partitioned | open_rgb_owned
allowed_detector_classes = []            # used with detector_partitioned
native_claimed_detector_classes = []     # classes reserved for native Hypercolor drivers
allow_low_confidence = false             # allow output for low-confidence non-HID/non-SMBus devices
```

---

## Troubleshooting

**No devices appear after enabling the driver.** The ownership mode defaults to
`disabled`. Set `mode = "open_rgb_owned"` (or `detector_partitioned`) and restart.

**"OpenRGB endpoint discovery failed."** Hypercolor cannot reach the server. Confirm
OpenRGB is running with `--server` and that the port matches `endpoints` in config. Check
logs with `RUST_LOG=hypercolor_driver_openrgb=debug`.

**Controller discovered but output-disabled.** Run `hypercolor devices info <id>` and
check the `disabled_reason` metadata field. Common causes: ownership mode excludes the
detector class, identity confidence is low (no serial or location string), the controller
has no per-LED writable mode, or zone and LED counts do not match.

**"detector_partition_confirmed must be true."** You configured a partition or native
claims without setting `detector_partition_confirmed = true`. Set it only after verifying
that OpenRGB's plugin configuration matches your intended partition.

**Flicker or frame corruption on SMBus devices.** Both Hypercolor and OpenRGB are writing
to the same SMBus controller simultaneously. Use `native_claimed_detector_classes =
["smbus"]` with `detector_partition_confirmed = true`, or disable OpenRGB's i2c-smbus
plugin for those devices.

**Controllers disappear after OpenRGB restarts.** Run `hypercolor devices discover
--target openrgb` to rescan. The bridge remaps by fingerprint when it receives an OpenRGB
device-list-updated notification, but an explicit discovery pass is required after a full
OpenRGB restart.

![Device discovery in the Hypercolor web UI](/img/ui/ui-devices.webp)

---

## See also

- [@/hardware/compatibility.md](@/hardware/compatibility.md) — check whether a native
  driver already covers your hardware before using the bridge
- [@/hardware/conflicting-software.md](@/hardware/conflicting-software.md) — running
  Hypercolor alongside OpenRGB and avoiding simultaneous writes
- [@/hardware/usb-devices.md](@/hardware/usb-devices.md) — native USB/HID driver setup
- [@/hardware/smbus-i2c.md](@/hardware/smbus-i2c.md) — native ASUS Aura motherboard
  and DRAM access over SMBus
- [@/troubleshooting/devices-not-found.md](@/troubleshooting/devices-not-found.md) —
  general device discovery troubleshooting
