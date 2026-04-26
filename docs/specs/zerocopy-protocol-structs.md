# Typed Protocol Messages via `zerocopy` in `hypercolor-hal`

**Status:** Planned (blocked by in-flight HID + Razer stack work)
**Scope:** `hypercolor-hal` crate only
**Success criteria:** All Razer wire-format packets use `#[derive(FromBytes, IntoBytes)]` structs instead of manual offset indexing. Corsair Lighting Node Direct packets migrated as stretch. `just verify` passes. Zero `unsafe`. No behavioral changes.

## Motivation

The HAL protocol layer constructs and parses fixed-format USB/HID packets using manual offset indexing into raw byte arrays. This works but is fragile — wrong offsets produce silent bugs, fields are unnamed magic numbers, and endianness is handled ad-hoc with `.to_le_bytes()`.

The `zerocopy` crate (Google, 0.8.x) provides safe, zero-cost type punning via derive macros. A `#[repr(C)]` struct with `#[derive(FromBytes, IntoBytes)]` can be viewed as `&[u8]` and vice versa — no copies, no `unsafe`, compile-time layout guarantees.

### Prior Art

- **uchroma** (`~/dev/uchroma`) uses a `RazerReport` wrapper struct with method-based field access, but still indexes into a raw `[u8; 90]` internally. This plan goes one step further — the struct _is_ the packet.

### What zerocopy replaces

| Before                                                       | After                                       |
| ------------------------------------------------------------ | ------------------------------------------- |
| `packet[STATUS_OFFSET]`                                      | `report.status`                             |
| `packet[DATA_SIZE_OFFSET] = len as u8`                       | `report.data_size = len as u8`              |
| `packet[ARGS_OFFSET..ARGS_OFFSET + n].copy_from_slice(args)` | `report.args[..n].copy_from_slice(args)`    |
| `razer_crc(&packet)` (takes `&[u8; 90]`)                     | `razer_crc(&report)` (takes `&RazerReport`) |
| Manual `.to_le_bytes()` for multi-byte fields                | `U16<LittleEndian>` at the type level       |

### What zerocopy does NOT replace

- `Arc<Vec<u8>>` canvas sharing (ownership, not interpretation)
- `FrameInput<'a>` lifetime borrows (allocation avoidance, not byte layout)
- Reusable `Vec<ProtocolCommand>` (capacity reuse pattern)
- RGB color arrays `[[u8; 3]]` (already trivially byte-compatible)

---

## Wave 1: Foundation — Dependency + Razer Report Struct

### Task 1: Add `zerocopy` dependency

**Files:** `crates/hypercolor-hal/Cargo.toml`
**Depends on:** Nothing

Add `zerocopy = { version = "0.8", features = ["derive"] }` and explicit `zerocopy-derive` for parallel proc-macro compilation.

**Verify:** `cargo check -p hypercolor-hal`

### Task 2: Define `RazerReport` as a typed struct

**Files:** `crates/hypercolor-hal/src/drivers/razer/protocol.rs`
**Depends on:** Task 1

```rust
use zerocopy::{FromBytes, FromZeros, IntoBytes, KnownLayout, Immutable};
use zerocopy::byteorder::{U16, LittleEndian};

#[derive(FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub(crate) struct RazerReport {
    pub status: u8,
    pub transaction_id: u8,
    pub remaining_packets: U16<LittleEndian>,
    pub protocol_type: u8,
    pub data_size: u8,
    pub command_class: u8,
    pub command_id: u8,
    pub args: [u8; 80],
    pub crc: u8,
    pub reserved: u8,
}
```

Add `#[cfg(test)]` assertion: `assert_eq!(size_of::<RazerReport>(), 90)`.

Keep existing offset constants alive temporarily for incremental migration.

**Verify:** `cargo test -p hypercolor-hal` — size assertion passes

### Task 3: Migrate CRC to accept `&RazerReport`

**Files:** `crates/hypercolor-hal/src/drivers/razer/crc.rs`
**Depends on:** Task 2

Use `.as_bytes()` to get `&[u8]` view, XOR bytes `[2..88]` as before. Add a new `razer_crc_report(&RazerReport)` function alongside the existing `razer_crc(&[u8; RAZER_REPORT_LEN])` — don't remove the array signature yet. The old signature is re-exported via `razer/mod.rs` and called directly in test files (`razer_protocol_tests.rs`, `razer_mamba_probe.rs`, `razer_hardware_smoke.rs`). Migrate internal callers to the typed version; deprecate the array version after all callers are updated.

**Verify:** Existing CRC tests pass with identical output

---

## Wave 2: Razer Packet Construction Migration

### Task 4: Migrate `build_packet_with_options()`

**Files:** `crates/hypercolor-hal/src/drivers/razer/protocol.rs`
**Depends on:** Task 3

Replace:

```rust
let mut packet = [0_u8; RAZER_REPORT_LEN];
packet[1] = transaction_id;
packet[DATA_SIZE_OFFSET] = data_size;
// ...
```

With:

```rust
let mut report = RazerReport::new_zeroed();
report.transaction_id = transaction_id;
report.data_size = data_size;
report.command_class = command_class;
report.command_id = command_id;
report.args[..args.len()].copy_from_slice(args);
report.crc = razer_crc(&report);
// report.as_bytes().to_vec() for ProtocolCommand.data
```

**Verify:** All protocol tests pass, wire output byte-identical

### Task 5: Migrate `parse_response()`

**Files:** `crates/hypercolor-hal/src/drivers/razer/protocol.rs`
**Depends on:** Task 2
**Parallel with:** Task 4

Replace `data[STATUS_OFFSET]` indexing with:

```rust
// Use read_from_prefix — NOT read_from_bytes — because HID transport can
// return >90 byte buffers (report ID still attached from decode fallback).
let (report, _remainder) = RazerReport::read_from_prefix(data)
    .map_err(|_| ProtocolError::MalformedResponse("buffer too small for report".into()))?;
let status = map_status(report.status);

// Preserve existing bounds check: data_size > 80 is malformed
let data_size = usize::from(report.data_size);
if data_size > ARGS_LEN {
    return Err(ProtocolError::MalformedResponse(
        format!("data_size {} exceeds args capacity", data_size),
    ));
}
let payload = &report.args[..data_size];
```

Note: Use `ProtocolError::MalformedResponse` (not `InvalidResponse` — that variant doesn't exist).

**Verify:** Parse response tests pass unchanged

### Task 6: Remove deprecated offset constants

**Files:** `crates/hypercolor-hal/src/drivers/razer/protocol.rs`
**Depends on:** Tasks 4, 5

Delete `STATUS_OFFSET`, `DATA_SIZE_OFFSET`, `COMMAND_CLASS_OFFSET`, `COMMAND_ID_OFFSET`, `ARGS_OFFSET`, `ARGS_LEN`, `CRC_OFFSET`. Keep `RAZER_REPORT_LEN` only if referenced by transport layer size checks.

**Verify:** `cargo check -p hypercolor-hal` — no remaining references

---

## Wave 3: Frame Encoding Verification

### Task 7: Verify `encode_matrix()` works through migrated `build_packet_with_options()`

**Files:** `crates/hypercolor-hal/src/drivers/razer/protocol.rs`
**Depends on:** Task 4
**Parallel with:** Tasks 8, 9

The chunking logic is unchanged — each chunk calls `build_packet_with_options()`. Verify standard vs extended matrix header args still encode correctly through the struct.

**Verify:** Matrix encoding tests pass, wire output byte-identical

### Task 8: Verify `encode_linear()`

**Files:** `crates/hypercolor-hal/src/drivers/razer/protocol.rs`
**Depends on:** Task 4
**Parallel with:** Tasks 7, 9

50-byte padded args for linear strips flow through `build_packet_with_options()`.

**Verify:** Linear encoding tests pass

### Task 9: Verify `encode_scalar()`

**Files:** `crates/hypercolor-hal/src/drivers/razer/protocol.rs`
**Depends on:** Task 4
**Parallel with:** Tasks 7, 8

Standard and extended scalar args. Already goes through `build_packet_with_options()`.

**Verify:** Scalar encoding tests pass

---

## Wave 4: Corsair Protocol Structs (Stretch — Lighting Node only)

> **Scoping note (from cross-model review):** LINK and LCD protocols have deeply variable
> payloads with offset-heavy parsing logic (endpoint open/close sequences, chunked color
> writes, LCD report framing). A small header struct doesn't meaningfully reduce their
> complexity. Only Lighting Node's fixed 65-byte Direct packet is a clean zerocopy candidate.
> LINK and LCD are out of scope for this plan — revisit if a pattern emerges.

### Task 10: Define typed struct for Lighting Node Direct packet

**Files:** `crates/hypercolor-hal/src/drivers/corsair/lighting_node/protocol.rs`
**Depends on:** Wave 2 complete (pattern proven)

65-byte Direct packet has a clean fixed layout:

```rust
#[derive(FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
struct LnDirectPacket {
    padding: u8,       // 0x00
    packet_id: u8,     // 0x32
    channel: u8,
    start_led: u8,
    led_count: u8,
    color_channel: u8, // 0=R, 1=G, 2=B
    values: [u8; 50],
    tail: [u8; 9],     // padding to 65
}
```

**Verify:** Lighting Node tests pass

---

## Wave 5: Optimization Polish

### Task 11: Explore `CommandBuffer` integration with typed structs

**Files:** `crates/hypercolor-hal/src/protocol.rs`, Razer protocol
**Depends on:** Wave 3 complete

`CommandBuffer::push_fill` currently takes a closure `FnOnce(&mut Vec<u8>)`. Could evolve to add a `push_struct<T: IntoBytes>` method that writes directly from `.as_bytes()`, avoiding the `.as_bytes().to_vec()` intermediate copy. Measure before changing.

**Verify:** Benchmarks show no regression, `just verify` passes

### Task 12: Port uchroma's optimized CRC

**Files:** `crates/hypercolor-hal/src/drivers/razer/crc.rs`
**Depends on:** Task 3

uchroma uses u64-width XOR accumulation with horizontal fold — faster for the 86-byte range. Port it to accept `&RazerReport` via `.as_bytes()`.

**Verify:** CRC tests pass, output byte-identical

---

## Summary

| Wave | Tasks   | Parallelism   | Theme                              |
| ---- | ------- | ------------- | ---------------------------------- |
| 1    | 1, 2, 3 | Sequential    | Foundation: dep + struct + CRC     |
| 2    | 4, 5, 6 | 4 ∥ 5, then 6 | Core: build + parse migration      |
| 3    | 7, 8, 9 | All parallel  | Frame encoders (verification pass) |
| 4    | 10      | Single task   | Corsair Lighting Node (stretch)    |
| 5    | 11, 12  | Parallel      | Optimization polish                |

**Core value:** Waves 1–3 (6 tasks, ~1 session)
**Stretch:** Wave 4 (Lighting Node only — LINK/LCD out of scope)
**Polish:** Wave 5 (opportunistic)

## Compatibility Notes

- `zerocopy` 0.8 is fully compatible with `#![forbid(unsafe_code)]`
- All `unsafe` is internal to the zerocopy crate
- `#[repr(C)]` required (not `#[repr(packed)]` — Razer report has no alignment gaps at u8/U16 boundaries)
- `FromZeros::new_zeroed()` replaces `[0_u8; 90]` initialization (requires `FromZeros` import)
- `FromBytes::read_from_prefix()` for response parsing (not `read_from_bytes` — HID buffers may be >90 bytes)
- Wire format is byte-identical before and after migration

## Review History

- **2026-03-09:** Cross-model review by GPT-5.4 via Codex CLI. 5 findings incorporated:
  - Fixed `read_from_bytes` → `read_from_prefix` for HID buffer compatibility
  - Descoped Corsair LINK/LCD (only Lighting Node Direct packet is a clean fit)
  - Preserved `data_size > 80` bounds check in parse_response
  - Fixed API references: `FromZeros` import, `MalformedResponse` variant, `push_fill` signature
  - CRC migration uses additive overload instead of hard signature swap
