# 34 -- Device Fingerprints

> Stable identity across reboots, cable swaps, and IP changes. A fingerprint is a promise that the same physical hardware maps to the same `DeviceId` forever.

_Source: [`hypercolor-types/src/device.rs`](../../crates/hypercolor-types/src/device.rs) (`DeviceIdentifier`, `DeviceFingerprint`, `stable_device_id`). Per-driver construction in the USB, SMBus, WLED, Hue, Nanoleaf, and ROLI Blocks scanner/backend modules._

---

## Table of Contents

1. [Overview](#1-overview)
2. [Fingerprint Format](#2-fingerprint-format)
3. [Per-Driver Derivation](#3-per-driver-derivation)
4. [DeviceId Generation](#4-deviceid-generation)
5. [Registry Deduplication](#5-registry-deduplication)
6. [Stability Guarantees](#6-stability-guarantees)
7. [Collision Analysis](#7-collision-analysis)
8. [Edge Cases](#8-edge-cases)

---

## 1. Overview

When a scanner discovers a physical device, it constructs a `DeviceIdentifier` from
transport-level data (USB descriptors, MAC address, bridge ID, etc.). The identifier's
`fingerprint()` method produces a `DeviceFingerprint` -- a short, deterministic string
that serves as the deduplication key in the `DeviceRegistry`. From that string, a
deterministic `DeviceId` (UUIDv8) is derived via dual-lane FNV-1a hashing.

The critical invariant: **the same physical device always produces the same fingerprint,
regardless of which USB port it's plugged into, which IP it acquired, or how many
reboots have occurred**, as long as the stable identity source (serial number, MAC, bridge
ID) hasn't changed.

## 2. Fingerprint Format

All fingerprints follow a prefixed string format:

```
<transport>:<discriminator>
```

The transport prefix is one of: `usb`, `smbus`, `net`, `hue`, `nanoleaf`, `bridge`.

The discriminator portion varies by transport but always uses the most stable identity
source available for that transport. The full string is passed through a 128-bit FNV-1a
hash to produce a UUIDv8-encoded `DeviceId`.

## 3. Per-Driver Derivation

### 3.1 USB HID (Razer, Corsair, Lian Li, Dygma, QMK, PrismRGB)

Constructed by the USB scanner in `hypercolor-core/src/device/usb_scanner.rs`.

| Field        | Source                                     |
| ------------ | ------------------------------------------ |
| `vendor_id`  | USB descriptor `idVendor`                  |
| `product_id` | USB descriptor `idProduct`                 |
| `serial`     | USB descriptor `iSerialNumber` (optional)  |
| `usb_path`   | Kernel topology path via `nusb` (fallback) |

**Format:** `usb:<vid>:<pid>:<stable_key>`

The stable key prefers `serial` when present. If the device reports no serial number,
the USB topology path (e.g. `usb-0000:00:14.0-2`) is used as a fallback.

**Examples:**

- `usb:1532:0084:PM2305A00012345` (Razer with serial)
- `usb:16d5:1f01:usb-0000:00:14.0-2` (no serial, path fallback)

### 3.2 SMBus (ASUS Aura)

Constructed by the SMBus scanner in `hypercolor-core/src/device/smbus_scanner.rs`.

| Field      | Source                                |
| ---------- | ------------------------------------- |
| `bus_path` | Linux device path (e.g. `/dev/i2c-9`) |
| `address`  | 7-bit I2C slave address               |

**Format:** `smbus:<bus_path>:<address_hex>`

**Example:** `smbus:/dev/i2c-9:40`

### 3.3 WLED

Constructed by both the WLED scanner (`wled/scanner.rs`) and the WLED backend cache
(`wled/backend/cache.rs`). Both paths produce identical fingerprints for the same device.

| Priority        | Source                        | Prefix                |
| --------------- | ----------------------------- | --------------------- |
| 1 (preferred)   | MAC address from `/json/info` | `net:<mac>`           |
| 2 (fallback)    | mDNS hostname                 | `net:wled:<hostname>` |
| 3 (last resort) | IP address                    | `net:wled:<ip>`       |

**Format:** `net:<mac_lowercase>` or `net:wled:<hostname>` or `net:wled:<ip>`

**Examples:**

- `net:a4:cf:12:34:ab:cd` (MAC available)
- `net:wled:studio-strip.local` (no MAC, hostname fallback)
- `net:wled:10.0.0.42` (neither MAC nor hostname)

### 3.4 Philips Hue

Constructed in `hypercolor-driver-hue/src/types.rs`. One fingerprint per bridge;
individual lights are modeled as zones within the bridge device, not as separate devices.

| Field       | Source                                                      |
| ----------- | ----------------------------------------------------------- |
| `bridge_id` | Hue bridge config `bridgeid` (MAC-derived, stable for life) |

**Format:** `hue:<bridge_id>`

**Example:** `hue:001788FFFE123456`

Note: `DeviceIdentifier::HueBridge` carries a `light_id` field and produces a
`hue:<bridge_id>:<light_id>` fingerprint. The current scanner uses the bridge-level
format. The per-light variant exists for future use when individual light control is
promoted to first-class device status.

### 3.5 Nanoleaf

Constructed in `hypercolor-driver-nanoleaf/src/types.rs`. The stable key is a
`device_key` derived by the scanner's `normalized_device_key()` function.

| Priority        | Source                                                 |
| --------------- | ------------------------------------------------------ |
| 1 (preferred)   | `device_id` from Nanoleaf API (hardware serial)        |
| 2 (fallback)    | `serial_no` from API info response                     |
| 3               | Device name (lowercased, spaces replaced with hyphens) |
| 4 (last resort) | `ip:<address>`                                         |

**Format:** `nanoleaf:<device_key>`

**Examples:**

- `nanoleaf:s19124c4321` (hardware serial)
- `nanoleaf:ip:10.0.0.50` (last resort)

### 3.6 ROLI Blocks (via blocksd bridge)

Constructed in `hypercolor-core/src/device/blocks/scanner.rs`.

| Field | Source                                       |
| ----- | -------------------------------------------- |
| `uid` | Numeric device UID from the blocksd REST API |

**Format:** `bridge:blocksd:<uid>`

**Example:** `bridge:blocksd:42`

### 3.7 External Bridge Devices (Generic)

The `DeviceIdentifier::Bridge` variant handles devices managed by external bridge
services (e.g. OpenLinkHub for Corsair iCUE-class hardware).

**Format:** `bridge:<service>:<device_serial>`

**Example:** `bridge:openlinkhub:ABC1234`

### Summary Table

| Driver                 | Transport  | Stable ID Source           | Format                          | Stability                         |
| ---------------------- | ---------- | -------------------------- | ------------------------------- | --------------------------------- |
| USB HID (all families) | `usb`      | Serial number or USB path  | `usb:<vid>:<pid>:<key>`         | Strong (serial) / Weak (path)     |
| ASUS Aura SMBus        | `smbus`    | Bus path + address         | `smbus:<path>:<addr>`           | Moderate                          |
| WLED                   | `net`      | MAC, hostname, or IP       | `net:<mac>` / `net:wled:<host>` | Strong (MAC) / Weak (IP)          |
| Philips Hue            | `hue`      | Bridge ID                  | `hue:<bridge_id>`               | Strong                            |
| Nanoleaf               | `nanoleaf` | Device ID, serial, or name | `nanoleaf:<key>`                | Strong (serial) / Moderate (name) |
| ROLI Blocks            | `bridge`   | blocksd UID                | `bridge:blocksd:<uid>`          | Strong                            |
| External bridge        | `bridge`   | Service + device serial    | `bridge:<svc>:<serial>`         | Strong                            |

## 4. DeviceId Generation

`DeviceFingerprint::stable_device_id()` converts the fingerprint string into a
deterministic UUIDv8 via dual-lane FNV-1a:

1. Two 64-bit FNV-1a hashes are computed over the fingerprint bytes with different
   offset bases (`0xCBF29CE484222325` and `0x8422_2325_CBF2_9CE4`).
2. The two halves are concatenated into 16 bytes.
3. RFC 9562 UUIDv8 version and variant bits are applied (version nibble `0x8`, variant
   `0b10`).
4. The result is wrapped in `DeviceId`.

This gives 122 effective bits of identity (128 minus 6 bits reserved by UUID formatting).
The same fingerprint string always produces the same `DeviceId`, allowing scanners and
backends to independently derive a canonical ID without shared runtime state.

## 5. Registry Deduplication

`DeviceRegistry` maintains a bidirectional index between fingerprints and device IDs:

- `fingerprints: HashMap<DeviceFingerprint, DeviceId>` -- dedup index
- `id_to_fingerprint: HashMap<DeviceId, DeviceFingerprint>` -- reverse index for cleanup

When a scanner produces a `DiscoveredDevice`:

1. The registry checks `fingerprints` for an existing entry.
2. If found and the `DeviceId` still exists in the primary map, the existing device's
   metadata is updated in place and the **original** `DeviceId` is returned.
3. If the fingerprint entry points to a stale ID, the index entry is cleaned up and a
   new device is registered.
4. For devices registered without a scanner fingerprint, a fallback fingerprint is
   generated from the UUID itself (`DeviceFingerprint(uuid.to_string())`).

An additional guard detects `DeviceId` collisions (possible when a scanner's
`stable_device_id()` collides with an existing entry for a different fingerprint). In
that case the registry allocates a fresh random `DeviceId` and logs a warning.

## 6. Stability Guarantees

**Strong stability** -- fingerprint survives reboots, cable swaps, IP changes, and
firmware updates:

- USB HID with serial number (device-burned unique identifier)
- Hue bridge (MAC-derived bridge ID, immutable)
- Nanoleaf with hardware serial
- ROLI Blocks (blocksd-assigned UID persists across reconnects)
- External bridge devices (serial assigned by bridge service)

**Moderate stability** -- fingerprint survives reboots but may change under specific
hardware reconfiguration:

- SMBus (stable unless the motherboard's I2C bus numbering changes, which can happen
  after BIOS updates or PCI topology changes)
- Nanoleaf with name-derived key (changes if the user renames the device)
- WLED with hostname fallback (changes if hostname is reconfigured)

**Weak stability** -- fingerprint may change on reconnect or network changes:

- USB HID without serial number (path-based; changes when plugged into a different port)
- WLED with IP fallback (changes on DHCP lease renewal)

## 7. Collision Analysis

**FNV-1a hash collisions.** The 122-bit effective keyspace (2^122 possible UUIDs) makes
accidental collisions astronomically unlikely for any realistic device count. A fleet of
10,000 devices has a collision probability of roughly 2^{-102}.

**Semantic collisions.** Two different physical devices could produce the same fingerprint
string only if their stable identity source is identical. Realistic scenarios:

- Two USB devices of the same VID:PID without serial numbers, plugged into the same port
  sequentially (not simultaneously) -- they share a fingerprint intentionally, because
  the system cannot distinguish them.
- Two WLED devices with identical MAC addresses -- a hardware defect or cloned firmware.
  The registry would merge them into one logical device.

**Cross-transport collisions.** Impossible by construction: the transport prefix
(`usb:`, `net:`, `hue:`, etc.) guarantees fingerprints from different transports never
collide.

## 8. Edge Cases

**USB device without serial number.** Falls back to the kernel USB topology path. This
fingerprint is port-specific: moving the device to a different USB port produces a new
identity. The `"unknown"` sentinel is used if neither serial nor path is available,
which would cause all such devices of the same VID:PID to share a fingerprint. In
practice, `nusb` always provides a path on Linux.

**WLED device with changed hostname.** If the device was previously fingerprinted by MAC
(the common case), a hostname change has no effect. If the device was fingerprinted by
hostname (no MAC available, typically older WLED firmware), the device appears as a new
device after the hostname change.

**WLED device with only IP.** The weakest fingerprint tier. A DHCP lease change produces
a new identity. Operators should ensure WLED devices either report a MAC (firmware 0.14+)
or have a static IP / DHCP reservation.

**Nanoleaf device rename.** If the device was fingerprinted by `device_id` or
`serial_no` (the common case), renaming has no effect. If only the name was available
during initial discovery, the device appears as new after a rename.

**Hue bridge replacement.** A new bridge has a different `bridge_id`. All lights under
the old bridge become orphaned entries in the registry. The operator must manually remove
stale entries or let the vanish-detection sweep handle them.

**SMBus bus renumbering.** Kernel I2C bus numbering can shift after a BIOS update or PCI
topology change. When this happens, the bus path component changes and the device appears
as new. This is inherent to the SMBus transport and cannot be mitigated without a
secondary stable identifier (which SMBus controllers do not provide).

**blocksd restart.** The blocksd daemon assigns stable UIDs that persist across restarts.
A device only loses its identity if the blocksd database is reset.
