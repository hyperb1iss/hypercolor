# 89 -- Experimental Driver Label and Device Verification

> The device database marks 184 devices `supported`, but nothing records
> which of them anyone has run on real hardware. Wave after wave of new
> drivers will be written from specs and captures by people who do not own
> the device. This spec adds two records: a runtime stability label that
> says a driver or protocol has not run on real hardware yet, and a
> `verification` field in the device database that says how each device
> was confirmed. It also states the rule that keeps untested code safe to
> ship: a protocol with a known safety hazard is not registered at all.

**Status:** Implemented. The stability label ships in the driver
contracts; the `verification` field ships in the device database and the
compatibility matrix.
**Crates:** `hypercolor-types` (vocabulary and API fields),
`hypercolor-driver-api` (driver descriptor label), `hypercolor-hal`
(descriptor label and derivation), `hypercolor-core` (unclaimed
inventory), `hypercolor-daemon` (API reporting), `hypercolor` (CLI),
`hypercolor-ui` (driver settings hint)
**Data:** `data/drivers/vendors/*.toml`, `scripts/gen-compat.ts`
**Related:** Spec 53 §8 (Corsair transport safety)
**Author:** Nova
**Date:** 2026-11-05

---

## Table of Contents

1. [Overview](#1-overview)
2. [Stability Label](#2-stability-label)
3. [Device Verification](#3-device-verification)
4. [How the Two Records Relate](#4-how-the-two-records-relate)
5. [Unsafe Means Unregistered](#5-unsafe-means-unregistered)
6. [Promotion Path](#6-promotion-path)
7. [Implementation Map](#7-implementation-map)
8. [Testing](#8-testing)
9. [References](#9-references)

---

## 1. Overview

Two questions used to share one word. "Supported" in the device database
meant "a driver exists for this device", and readers took it to mean "this
works". A user with an untested device had no way to know their report
would be the first.

This spec separates them:

| Question | Record | Lives in |
| --- | --- | --- |
| Has this driver or protocol code ever run on real hardware? | Stability label: `stable` or `experimental` | Driver and protocol descriptors, reported over the API |
| How was this specific device confirmed? | `verification`: `hardware`, `community`, or `unverified` | Device database rows, rendered in the compatibility matrix |

Neither record changes what runs. Experimental drivers are enabled by
default and claim devices exactly like stable ones. Nobody opts in to
anything. Code that could hurt a user's machine never reaches them,
because it is not registered (§5).

### 1.1 Non-goals

- No runtime gate, opt-in flag, or extra config key. An earlier draft of
  this spec gated experimental protocols behind a per-driver opt-in; the
  maintainer rejected it as friction without safety value, since the
  dangerous cases are already handled by not registering them.
- No UI emphasis. The web UI does not badge experimental devices or
  drivers (§2.5).
- No automatic promotion from telemetry. Promotion is a reviewed change
  (§6).

---

## 2. Stability Label

### 2.1 Vocabulary

`hypercolor_types::device::DriverStability` is a two-value enum serialized
in `snake_case`:

| Value | Meaning |
| --- | --- |
| `stable` (default) | Run on real hardware by a maintainer, or reported working by a user, or shipped before this spec existed (§4) |
| `experimental` | Built from a spec, captures, or reference code, and not yet run on real hardware |

Every field that carries it is `#[serde(default)]` and omitted from the
wire when it holds the default (`stable`, or `false` for the device
booleans). Documents written before this spec read as `stable`, and a
client that re-serializes an older daemon's payload, such as
`hypercolor drivers list --json`, reproduces it unchanged.

### 2.2 Where the label lives

| Type | Field | Set by |
| --- | --- | --- |
| `hypercolor_driver_api::DriverDescriptor` | `stability` | `DriverDescriptor::new` sets `Stable`; the const builder `.experimental()` sets `Experimental` |
| `hypercolor_types::device::DriverModuleDescriptor` | `stability` | Copied from the driver descriptor, or derived for HAL families (§2.3) |
| `hypercolor_types::device::DriverProtocolDescriptor` | `stability` | Copied from the HAL `DeviceDescriptor` that contributes the protocol |
| `hypercolor_hal::registry::DeviceDescriptor` | `stability` | Required on every descriptor literal; the const builder `.experimental()` relabels a descriptor built by a shared helper |

`.experimental()` only sets the label. It never touches
`default_enabled`, so an experimental network driver ships enabled:

```rust
pub static DESCRIPTOR: DriverDescriptor = DriverDescriptor::new(
    "lifx",
    "LIFX",
    DriverTransportKind::Network,
    true,
    false,
)
.experimental();
```

A HAL descriptor sets `stability: DriverStability::Experimental` beside
its other fields, or calls `.experimental()` on a descriptor a shared
helper built. Descriptors that shipped before this spec set
`DriverStability::Stable`. The first experimental descriptors are seven
Razer accessories registered from captures without a hardware run: the
Kraken 7.1 V2, Kraken Tournament Edition, Kraken Kitty V2, and Kraken 7.1
Chroma headsets, the Tomahawk ATX and Raptor 27, and the Core X Chroma.
The Razer module stays stable around them.

### 2.3 Derived module label for HAL families

The HAL database derives one driver module per family. Its label follows
the family's descriptors:

| Family contents | Module label | Protocol labels |
| --- | --- | --- |
| Every descriptor experimental (a new family) | `experimental` | All `experimental` |
| At least one stable descriptor | `stable` | Each protocol carries its own descriptor's label |

So a Corsair family with shipped Bragi descriptors and a future set of
untested NXP/CUE descriptors stays a stable module whose catalog lists
the NXP/CUE protocols as `experimental`. Device identity, the `corsair`
driver id, and presentation do not change, so promoting a protocol later
moves nothing.

### 2.4 Consistency rule

A connected device is labeled through the `protocol_id` in its
`DeviceOrigin`, so every HAL descriptor sharing a protocol id must carry
one label. `ProtocolDatabase::stability_conflicts` lists the protocol ids
that break this, and a database test asserts the registered set has none.
An experimental variant of an existing protocol takes its own protocol id.

### 2.5 What clients see

| Surface | What it shows |
| --- | --- |
| `GET /api/v1/drivers` | `descriptor.stability` and `protocols[].stability` |
| `GET /api/v1/devices` | `DeviceSummary.experimental`: true when the device's module or protocol is labeled experimental |
| Unclaimed USB inventory | `UnclaimedDevice.experimental`: true when the descriptor that could claim the device is experimental (omitted when false) |
| `hypercolor drivers list` | A Tier column: `stable`, `experimental`, or `stable +N experimental` |
| `hypercolor devices list` | `(experimental)` after the driver id |
| Web UI | Deliberately quiet. A driver settings row for an experimental module carries one muted line, "Untested on real hardware". No badge, chip, or accent anywhere |

All of these come from the shared contracts in `hypercolor-types`; no
client mirrors the types by hand.

---

## 3. Device Verification

### 3.1 Values

Each `[[devices]]` row in `data/drivers/vendors/*.toml` may carry
`verification`:

| Value | Meaning | Evidence required |
| --- | --- | --- |
| `hardware` | A maintainer ran this device on real hardware with the current driver | A commit, spec, or PR that records the run, or local diagnostics showing frames delivered |
| `community` | A user reported the device working | A closed GitHub issue or discussion on `hyperb1iss/hypercolor` in which a user other than the maintainer confirms it works |
| `unverified` (default) | Encoder, spec, or capture only | None |

The key is optional and defaults to `unverified`, so the honest answer is
also the one a contributor gets by leaving it out. A row may only claim
`hardware` or `community` when it names a `driver`: a device without a
driver has nothing to verify. A `blocked` row may be `hardware` when a
maintainer ran it and found the block (the Dygma Defy).

When one row covers several products (a shared PID), `hardware` or
`community` needs evidence for the product the row is named after; a
sibling sharing the PID does not count.

### 3.2 What does not count as evidence

- The device appearing in a protocol catalog or the unclaimed inventory.
- Packet captures or a reference implementation working on someone else's
  machine. Those inform the encoder; they do not verify Hypercolor.
- A maintainer closing an issue without a user's confirmation.
- A spec's acceptance criterion that says "validated on hardware" as a
  goal rather than a record.

### 3.3 Where it shows

`just compat` emits `verification` for every device in
`data/compat/compatibility.json`, adds per-vendor and total counts of
tested supported devices, and renders a Verified column in the docs-site
matrix and the README summary. `just compat-check` gates the outputs in CI
as before.

---

## 4. How the Two Records Relate

New drivers start experimental in code and `unverified` in the database.
They meet again at promotion (§6).

Drivers that shipped before this spec are labeled `stable` in code even
where their database rows are `unverified`. Flipping them to
experimental would not change runtime behavior and would only add noise;
the database row is where a reader learns whether that exact device was
tested. The compatibility matrix makes the gap visible: most shipped rows
are `unverified` today.

The label is per protocol; verification is per device row. A stable
protocol can serve unverified rows (a new PID added to a proven protocol),
and that is expected.

---

## 5. Unsafe Means Unregistered

The label is for code that is untested but believed safe. Code with a
known safety hazard is not registered at all until the hazard is resolved
and verified on hardware. Registration, not a runtime flag, is the
boundary.

The standing example is Spec 53 §8.4 and §8.5: some Corsair NXP paths
claim interrupt endpoints directly, which on Linux can detach `usbhid` and
take away keyboard or mouse input. Those encoders exist and have tests,
but no descriptor routes a device to them. A future change may register
them, labeled experimental, only once the transport path is shown not to
detach the kernel input driver.

A reviewer applies one question to every new descriptor: if this encoder
is wrong, can the device or the host end up worse than "the lights look
wrong"? Bricked firmware, lost input, a wedged bus, or a stuck fan curve
mean the descriptor waits. Wrong colors, wrong LED counts, or no output
mean it can ship labeled experimental.

---

## 6. Promotion Path

### 6.1 Database rows

```text
unverified -> community -> hardware
```

Rows move up one evidence step at a time, but `unverified` may jump
straight to `hardware` when a maintainer runs the device.

| Step | Who may make it | What the change must include |
| --- | --- | --- |
| `unverified` to `community` | Any contributor; a maintainer merges | The issue or discussion URL in the PR body, and a short note in the row's `notes` such as "Reported working in #362" |
| any to `hardware` | A maintainer with the device | The device, firmware version, OS, Hypercolor version, and what was exercised, in the commit body or PR body |
| any to `unverified` | A maintainer | Why: a protocol rewrite that changed the wire path, or evidence that turned out not to apply |

A regression report does not demote a row by itself; it is a bug against
a verified device, and the row stays verified while the bug is fixed.

### 6.2 Stability label

A protocol flips from `experimental` to `stable` when at least one
database row it serves reaches `community` or `hardware`. The flip and
the row change land in the same PR, which cites the evidence. A wholly
experimental HAL family becomes a stable module automatically once any of
its descriptors flips. A network driver flips by dropping
`.experimental()` from its descriptor.

---

## 7. Implementation Map

| Concern | Location |
| --- | --- |
| Vocabulary | `crates/hypercolor-types/src/device.rs` (`DriverStability`, descriptor fields) |
| API fields | `crates/hypercolor-types/src/api/devices.rs` (`DeviceSummary.experimental`, `UnclaimedDevice.experimental`) |
| Driver descriptor builder | `crates/hypercolor-driver-api/src/descriptor.rs` |
| HAL descriptor field | `crates/hypercolor-hal/src/registry.rs` |
| HAL derivation and consistency check | `crates/hypercolor-hal/src/database.rs` (`derive_module_descriptors`, `protocol_descriptor_for`, `stability_conflicts`) |
| Unclaimed inventory label | `crates/hypercolor-core/src/device/unclaimed.rs`, `usb_scanner.rs` |
| Device label in the API | `crates/hypercolor-daemon/src/network.rs` (`device_is_experimental`) and `api/devices/mod.rs` |
| CLI | `crates/hypercolor-cli/src/commands/drivers.rs` (`tier_label`), `devices.rs` |
| Web UI | `crates/hypercolor-ui/src/driver_settings.rs`, `components/settings_sections/discovery.rs` |
| Database field and generator | `data/drivers/README.md`, `data/drivers/vendors/*.toml`, `scripts/gen-compat.ts` |

---

## 8. Testing

| Behavior | Test |
| --- | --- |
| `.experimental()` sets only the label; modules stay enabled by default | `crates/hypercolor-driver-api/tests/descriptor_stability_tests.rs` |
| Serde defaults and wire names | `crates/hypercolor-types/tests/driver_stability_tests.rs` |
| Family derivation, protocol labels, consistency rule, the untested Razer accessories labeled inside a stable module | `crates/hypercolor-hal/tests/driver_stability_tests.rs` |
| Unclaimed inventory claims experimental descriptors like stable ones and keeps the label | `crates/hypercolor-core/tests/unclaimed_device_tests.rs` |
| Experimental modules enabled by default, ordinary enable flag, device label, `/drivers` and `/devices` fields | `crates/hypercolor-daemon/tests/driver_stability_tests.rs` |
| Compiled-in drivers keep the stable label and their defaults | `crates/hypercolor-driver-builtin/tests/builtin_registry_tests.rs` |
| CLI tier column | `crates/hypercolor-cli/tests/driver_tier_tests.rs` |
| UI hint and unchanged toggle rows | `crates/hypercolor-ui/tests/driver_settings_tests.rs` |
| Database field validation and generated outputs | `just compat-check` |

---

## 9. References

- Spec 53, Corsair Peripheral Protocol Driver, §8.4 Input Device Safety and
  §8.5 Kernel Driver Interaction.
- `data/drivers/README.md`, device database schema.
- `data/compat/compatibility.json`, the generated matrix that is the only
  authority for device counts.
