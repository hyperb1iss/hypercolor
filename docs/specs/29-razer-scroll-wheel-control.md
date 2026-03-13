# Spec 29: Razer Scroll Wheel Control

**Status:** Planned
**Scope:** HAL + Types (daemon API wiring deferred to follow-up)

## Overview

Add scroll wheel mode control to Hypercolor's Razer HAL driver. This is the
first non-RGB device feature, establishing the pattern for future device
configuration commands (DPI, polling rate, button remapping, etc.).

Razer mice with motorized scroll wheels (Basilisk V3 family, Naga V2 Pro) use
an electric actuator that can be commanded over USB to switch between tactile
(ratcheted) and free-spin scrolling modes. The protocol also supports "Smart
Reel" — automatic mode switching based on scroll velocity — and scroll
acceleration.

## Protocol Details

All scroll commands use the same `RazerReport` wire format (90-byte HID feature
report) as lighting commands, on command class `0x02` (device configuration).

### Command Table

| Feature          | Set CMD | Get CMD | Class  | Data Size | Args                        |
|------------------|---------|---------|--------|-----------|-----------------------------|
| Scroll mode      | `0x14`  | `0x94`  | `0x02` | `0x02`    | `[VARSTORE, mode]`          |
| Smart reel       | `0x17`  | `0x97`  | `0x02` | `0x02`    | `[VARSTORE, enabled]`       |
| Scroll accel     | `0x16`  | `0x96`  | `0x02` | `0x02`    | `[VARSTORE, enabled]`       |

### Argument Values

- **VARSTORE** = `0x01` (persist to device flash)
- **Scroll mode:** `0x00` = tactile (ratchet), `0x01` = free-spin
- **Smart reel:** `0x00` = off, `0x01` = on
- **Scroll acceleration:** `0x00` = off, `0x01` = on

### Supported Devices

| Device | PID(s) | Scroll Mode | Smart Reel | Accel |
|--------|--------|:-----------:|:----------:|:-----:|
| Basilisk V3 | `0x0099` | ✓ | ✓ | ✓ |
| Basilisk V3 35K | `0x00CB` | ✓ | ✓ | ✓ |
| Basilisk V3 Pro (Wired) | `0x00AA` | ✓ | ✓ | ✓ |
| Basilisk V3 Pro (Wireless) | `0x00AB` | ✓ | ✓ | ✓ |
| Basilisk V3 Pro 35K (Wired) | `0x00CC` | ✓ | ✓ | ✓ |
| Basilisk V3 Pro 35K (Wireless) | `0x00CD` | ✓ | ✓ | ✓ |
| Basilisk V3 Pro 35K Phantom (Wired) | `0x00D6` | ✓ | ✓ | ✓ |
| Basilisk V3 Pro 35K Phantom (Wireless) | `0x00D7` | ✓ | ✓ | ✓ |
| Basilisk V3 X HyperSpeed | `0x00B9` | ✓ | ✓ | ✓ |

> **Note:** Naga V2 Pro also supports scroll mode toggle (HyperScroll Pro
> Wheel) but uses an extended protocol with 6 modes + haptic feedback. Defer
> until we can capture packets for its specific command structure.

## Implementation Plan

### Layer 1: `hypercolor-types/src/device.rs`

Add a `DeviceFeatures` flags struct to `DeviceCapabilities`:

```rust
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceFeatures {
    /// Supports tactile/free-spin scroll wheel toggle.
    pub scroll_mode: bool,
    /// Supports Smart Reel auto-switching.
    pub scroll_smart_reel: bool,
    /// Supports scroll acceleration toggle.
    pub scroll_acceleration: bool,
}
```

Add `features: DeviceFeatures` field to `DeviceCapabilities` with
`#[serde(default)]` for backwards compat.

### Layer 2: `hypercolor-hal/src/drivers/razer/types.rs`

New constants and types:

```rust
// Command class for device configuration (non-lighting).
pub const COMMAND_CLASS_DEVICE: u8 = 0x02;

// Scroll wheel command IDs.
pub const COMMAND_SET_SCROLL_MODE: u8 = 0x14;
pub const COMMAND_GET_SCROLL_MODE: u8 = 0x94;
pub const COMMAND_SET_SCROLL_SMART_REEL: u8 = 0x17;
pub const COMMAND_GET_SCROLL_SMART_REEL: u8 = 0x97;
pub const COMMAND_SET_SCROLL_ACCELERATION: u8 = 0x16;
pub const COMMAND_GET_SCROLL_ACCELERATION: u8 = 0x96;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScrollMode {
    Tactile = 0x00,
    FreeSpin = 0x01,
}
```

### Layer 3: `hypercolor-hal/src/protocol.rs`

Extend `Protocol` trait with default-`None` methods:

```rust
fn encode_scroll_mode(&self, _mode: ScrollMode) -> Option<Vec<ProtocolCommand>> {
    None
}

fn encode_scroll_smart_reel(&self, _enabled: bool) -> Option<Vec<ProtocolCommand>> {
    None
}

fn encode_scroll_acceleration(&self, _enabled: bool) -> Option<Vec<ProtocolCommand>> {
    None
}
```

> **Design note:** Using `ScrollMode` in the trait signature means importing
> from `hypercolor-hal::drivers::razer::types` — but since `ScrollMode` is a
> device-agnostic concept (Logitech also has it), it belongs in
> `hypercolor-types`. Move it there.

### Layer 4: `hypercolor-hal/src/drivers/razer/protocol.rs`

Add `supports_scroll_features: bool` to `RazerProtocol` + builder:

```rust
pub const fn with_scroll_features(mut self) -> Self {
    self.supports_scroll_features = true;
    self
}
```

Implement encoding methods using `build_packet()` with `COMMAND_CLASS_DEVICE`:

```rust
fn encode_scroll_mode(&self, mode: ScrollMode) -> Option<Vec<ProtocolCommand>> {
    if !self.supports_scroll_features {
        return None;
    }
    self.build_packet(
        COMMAND_CLASS_DEVICE,
        COMMAND_SET_SCROLL_MODE,
        &[VARSTORE, mode as u8],
        true,
        Duration::ZERO,
    )
    .map(|cmd| vec![cmd])
}
```

Same pattern for smart reel (`0x17`) and acceleration (`0x16`).

Wire `supports_scroll_features` into `capabilities()` → `DeviceFeatures`.

### Layer 5: `hypercolor-hal/src/drivers/razer/devices.rs`

Update protocol factory functions for scroll-capable devices:

- `build_basilisk_v3_protocol()` — append `.with_scroll_features()`
- Basilisk V3 Pro group uses `razer_matrix_builder!` macro → need to either:
  - Add a separate factory function with `.with_scroll_features()`, or
  - Extend the macro to accept optional builder chain calls

Cleanest approach: create `build_basilisk_v3_pro_protocol()` that wraps the
macro-generated builder + `.with_scroll_features()`, and point the device group
at it. Same for the Basilisk V3 X HyperSpeed.

### Layer 6: Tests

`crates/hypercolor-hal/tests/razer_scroll.rs`:

- Verify `encode_scroll_mode(Tactile)` produces correct 90-byte report with
  class `0x02`, ID `0x14`, args `[0x01, 0x00]`
- Verify `encode_scroll_mode(FreeSpin)` → args `[0x01, 0x01]`
- Verify `encode_scroll_smart_reel(true/false)` → correct command IDs and args
- Verify `encode_scroll_acceleration(true/false)` → correct command IDs and args
- Verify non-scroll devices return `None` for all scroll methods
- Verify `capabilities().features.scroll_mode == true` for Basilisk V3

`crates/hypercolor-types/tests/device_features.rs`:

- Serde round-trip for `DeviceFeatures`
- Default is all-false
- Backwards compat: deserialize old `DeviceCapabilities` JSON (no `features`
  field) → features all default to false

## Deferred Work

- **Daemon REST API** — `POST /api/v1/devices/:id/scroll` endpoint
- **WebSocket events** — Scroll mode change notifications
- **UI controls** — Leptos toggle in device settings panel
- **TUI controls** — Ratatui scroll mode widget
- **Naga V2 Pro** — Extended scroll protocol with 6 modes + haptic
- **DPI / polling rate / button remapping** — Future `DeviceFeatures` expansion
- **Read-back** — `GET` commands to query current scroll state on connect
