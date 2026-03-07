# 18 -- Corsair Integration

> Two-phase Corsair strategy: bridge first for instant iCUE LINK support, native hub-and-spoke driver later for per-LED 60fps control.

**Status:** Draft
**Crate:** `hypercolor-hal`
**Module path:** `hypercolor_hal::drivers::corsair`
**Author:** Nova
**Date:** 2026-03-04

---

## Table of Contents

1. [Overview](#1-overview)
2. [Corsair Protocol Landscape](#2-corsair-protocol-landscape)
3. [OpenLinkHub Bridge Architecture](#3-openlinkhub-bridge-architecture)
4. [Bridge Discovery & Lifecycle](#4-bridge-discovery--lifecycle)
5. [Color Pipeline Limitations](#5-color-pipeline-limitations)
6. [Native iCUE LINK Protocol (Phase 2)](#6-native-icue-link-protocol-phase-2)
7. [HAL Integration](#7-hal-integration)
8. [Testing Strategy](#8-testing-strategy)

---

## 1. Overview

Corsair integration follows a two-phase strategy driven by protocol complexity and hardware availability:

| Phase | Approach | Scope | Dependency |
|-------|----------|-------|------------|
| **Phase 1** | OpenLinkHub REST bridge | Immediate iCUE LINK support | External: OpenLinkHub daemon |
| **Phase 2** | Native iCUE LINK driver | Per-LED 60fps direct control | None (standalone) |

**Target hardware (Bliss's inventory):**
- iCUE LINK System Hub (`0x0C3F`) — central daisy-chain bus controller
- iCUE LINK LCD AIO — connected downstream via LINK bus

Phase 1 provides working Corsair support with minimal implementation effort. Phase 2 eliminates the external dependency and enables full per-LED real-time control — but the hub-and-spoke enumeration, chunked color writes, and keepalive requirements make it a substantially larger effort.

---

## 2. Corsair Protocol Landscape

Corsair has five distinct protocol families across their product line. Understanding the landscape is important for scoping what we build vs. what we defer.

### 2.1 Protocol Family Map

| # | Family | Usage Page | Packet Size | Products | Priority |
|---|--------|-----------|-------------|----------|----------|
| 1 | **iCUE LINK** | `0xFF42` (usage `0x01`) | 513B write / 512B read | System Hub, LINK fans, AIOs | **High** (Phase 1+2) |
| 2 | Commander Core / XT | `0xFF42` (usage `0x01`) | 97B / 385B / 1025B (varies) | Commander Core, Core XT | Low |
| 3 | Lighting Node | bare interface | 65B write / 17B read | Node Pro, Commander Pro, LS100 | Low |
| 4 | Peripheral V2 | `0xFF42` (interface 1) | 65B or 1025B | K100, K70, M65, Dark Core | Low |
| 5 | Legacy Peripheral | `0xFFC2` | varies | K65, K95, Scimitar, M65 (pre-2020) | None |

**VID:** `0x1B1C` (Corsair) across all families.

### 2.2 Why iCUE LINK First

The iCUE LINK System Hub is the single USB endpoint for Corsair's modern hub-and-spoke architecture. All downstream devices — fans, pump heads, LCD panels — communicate over the proprietary LINK bus, not directly via USB. Driving the hub means driving every connected device.

The other protocol families (Commander Core, Lighting Node, V2 Peripherals) are separate USB devices with their own protocol stacks. They can be added as independent drivers in future specs without affecting the LINK implementation.

---

## 3. OpenLinkHub Bridge Architecture

Phase 1 uses [OpenLinkHub](https://github.com/jurkovic-nikola/OpenLinkHub) as an intermediary, exposing Corsair iCUE LINK devices via a REST API.

### 3.1 Bridge Backend

The bridge backend implements the HAL `DeviceBackend` trait (not the low-level `Protocol` trait), operating at a higher abstraction level:

```rust
/// OpenLinkHub bridge backend for Corsair iCUE LINK devices.
///
/// Discovers and controls Corsair devices via the OpenLinkHub
/// REST API. Higher-level than a raw protocol driver — OpenLinkHub
/// handles all USB communication, device enumeration, and
/// protocol negotiation internally.
pub struct CorsairBridgeBackend {
    /// HTTP client for OpenLinkHub API.
    client: reqwest::Client,
    /// Base URL (default: http://localhost:27003).
    base_url: String,
    /// Cached device list, refreshed on polling interval.
    devices: Vec<BridgeDevice>,
}
```

### 3.2 API Endpoints

| Operation | Method | Endpoint | Body |
|-----------|--------|----------|------|
| Health check | `GET` | `/api/` | — |
| List devices | `GET` | `/api/devices/` | — |
| Set color | `POST` | `/api/color` | `{ serial, profile, channel, ... }` |
| Set brightness | `POST` | `/api/brightness/gradual` | `{ serial, brightness: 0-100 }` |
| Set fan speed | `POST` | `/api/speed/manual` | `{ serial, channel, speed }` |
| LCD image | `POST` | `/api/lcd/image` | `{ serial, image_data }` |
| LCD rotation | `POST` | `/api/lcd/rotation` | `{ serial, rotation }` |

### 3.3 Device Discovery Mapping

OpenLinkHub's `GET /api/devices/` response maps to Hypercolor's `DeviceInfo`:

```
OpenLinkHub response          →  Hypercolor DeviceInfo
───────────────────────────      ──────────────────────
device.serial                 →  identifier (DeviceIdentifier::Bridge)
device.name                   →  display_name
device.product                →  model
device.led_channels           →  zone_count
device.led_count              →  total_leds
device.temperature_probes     →  capabilities (temperature sensing)
device.fan_rpm                →  capabilities (fan control)
```

---

## 4. Bridge Discovery & Lifecycle

### 4.1 Startup Sequence

```
┌──────────────────────────────────────────┐
│ 1. Probe GET /api/ on localhost:27003    │
│    ├── Success → OpenLinkHub available   │
│    └── Failure → skip Corsair backend    │
│                                          │
│ 2. GET /api/devices/ → enumerate         │
│    Map each device to DeviceIdentifier   │
│                                          │
│ 3. Register devices with HAL             │
│    DeviceIdentifier::Bridge {            │
│        service: "openlinkhub",           │
│        device_serial: "...",             │
│    }                                     │
│                                          │
│ 4. Start polling timer for device list   │
│    refresh (default: 5s interval)        │
└──────────────────────────────────────────┘
```

### 4.2 Graceful Degradation

- If OpenLinkHub is not running at startup, the Corsair bridge backend is silently skipped
- If OpenLinkHub goes down mid-session, existing device handles become stale; the backend marks devices as disconnected on the next failed poll
- Reconnection is automatic on the next successful health check

### 4.3 Device Identification

```rust
/// Bridge device identifier for externally-managed devices.
pub enum DeviceIdentifier {
    /// Direct USB device (VID/PID/serial).
    Usb { vid: u16, pid: u16, serial: String },
    /// Network device (IP/port).
    Network { address: SocketAddr },
    /// Bridge-managed device (service name + device ID).
    Bridge { service: String, device_serial: String },
}
```

---

## 5. Color Pipeline Limitations

### 5.1 Profile-Based vs. Direct Control

OpenLinkHub's REST API is **profile-based** — it maps named lighting profiles to device channels, not raw LED arrays. This means:

- **Static colors:** Fully supported via `POST /api/color`
- **Preset effects:** Supported via profile name mapping
- **Per-LED real-time control:** Not supported through REST API

### 5.2 Real-Time Control Status

Phase 1 routes Corsair lighting through OpenLinkHub's REST API for static color,
profile changes, and other non-streaming operations. Hypercolor does not ship a
dedicated high-frequency relay path here anymore.

Per-LED 60fps animation is deferred until the native USB backend lands in Phase
2. At that point, the effect engine will write frames directly through the
native Corsair transport instead of tunneling through a separate compatibility
service.

### 5.3 Decision Matrix

| Scenario | Transport | Latency | Per-LED |
|----------|-----------|---------|---------|
| Static color | REST API | ~10ms | No (zone-level) |
| Preset effect (breathing, rainbow) | REST API | ~10ms | No |
| Real-time animation (60fps) | Native USB backend (Phase 2) | TBD | Deferred |
| Brightness adjustment | REST API | ~10ms | No |
| Fan speed control | REST API | ~10ms | N/A |

### 5.4 Profile Preset Mapping

Hypercolor's built-in effects map to OpenLinkHub profile names:

| Hypercolor Effect | OpenLinkHub Profile |
|-------------------|-------------------|
| Solid color | `static` |
| Breathing | `breathing` |
| Rainbow wave | `rainbow` |
| Color cycle | `colorshift` |

---

## 6. Native iCUE LINK Protocol (Phase 2)

This section documents the wire protocol for future native driver implementation, eliminating the OpenLinkHub dependency.

### 6.1 USB Identification

| Field | Value |
|-------|-------|
| VID | `0x1B1C` |
| PID | `0x0C3F` (System Hub) |
| Interface | `0x00` |
| Usage Page | `0xFF42` |
| Usage | `0x01` |

### 6.2 Packet Geometry

```
Write buffer: 513 bytes
  [0]     = 0x00 (HID report ID)
  [1]     = 0x00 (reserved)
  [2]     = 0x01 (command present flag)
  [3..]   = command bytes + data
  [..512] = zero-padded

Read buffer: 512 bytes
  [0..2]  = header (3 bytes)
  [3..]   = response data

Max data per request: 508 bytes
```

### 6.3 Command Vocabulary

| Command | Bytes | Purpose |
|---------|-------|---------|
| Open endpoint | `0x0D, 0x01` | Open a data endpoint for read/write |
| Open color endpoint | `0x0D, 0x00` | Open endpoint for color writes |
| Close endpoint | `0x05, 0x01, 0x01` | Close current endpoint |
| Get firmware | `0x02, 0x13` | Query hub firmware version |
| Get device mode | `0x01, 0x08, 0x01` | Software vs hardware mode |
| Software mode | `0x01, 0x03, 0x00, 0x02` | Take control from firmware |
| Hardware mode | `0x01, 0x03, 0x00, 0x01` | Return control to firmware |
| Write (standard) | `0x06, 0x01` | First chunk of standard data |
| Write color (first) | `0x06, 0x00` | First chunk of color data |
| Write color (next) | `0x07, 0x00` | Subsequent color data chunks |
| Read | `0x08, 0x01` | Read endpoint data |

### 6.4 Endpoint Addresses

| Endpoint | Address | Data Type | Purpose |
|----------|---------|-----------|---------|
| Get devices | `0x36` | `0x21, 0x00` | Enumerate downstream devices |
| Get temperatures | `0x21` | `0x10, 0x00` | Read temperature sensors |
| Get fan speeds | `0x17` | `0x25, 0x00` | Read fan RPMs |
| Set fan speed | `0x18` | `0x07, 0x00` | Write fan speed targets |
| Set color | `0x22` | `0x12, 0x00` | Write LED color data |

### 6.5 Protocol Flow — Read Transaction

```
1. SendCommand(CLOSE_ENDPOINT, endpoint)     // clean state
2. SendCommand(OPEN_ENDPOINT, endpoint)      // open for reading
3. SendCommand(READ, {}, data_type)          // poll until response[4] matches
4. SendCommand(CLOSE_ENDPOINT, endpoint)     // release
```

### 6.6 Protocol Flow — Color Write (Chunked)

```
1. SendCommand(CLOSE_ENDPOINT, endpoint)
2. SendCommand(OPEN_COLOR_ENDPOINT, endpoint)    // 0x0D, 0x00
3. Chunk 0: SendCommand(WRITE_COLOR, chunk)      // 0x06, 0x00
4. Chunk N: SendCommand(WRITE_COLOR_NEXT, chunk) // 0x07, 0x00
5. SendCommand(CLOSE_ENDPOINT, endpoint)
```

Color data is packed as flat `RGBRGBRGB...` across all downstream devices. The hub distributes segments to each spoke based on enumeration order and LED counts. Max 508 bytes per chunk.

### 6.7 Downstream Device Enumeration

Response from `GET_DEVICES` endpoint:

```
[0..5]   header
[6]      channel_count

Per-channel record:
  [pos+0..1]  unknown metadata
  [pos+2]     device_type
  [pos+3]     device_model
  [pos+4..6]  unknown metadata
  [pos+7]     device_id_length (0 = empty slot, skip)
  [pos+8..]   device serial string (device_id_length bytes)
  advance: pos += 8 + device_id_length
```

### 6.8 Known Downstream Device Types

| Type | Model | Name | LEDs |
|------|-------|------|------|
| `0x01` | `0x00` | iCUE LINK QX RGB Fan | 34 |
| `0x02` | `0x00` | iCUE LINK LX RGB Fan | 18 |
| `0x03` | `0x00` | iCUE LINK RX RGB MAX Fan | 8 |
| `0x04` | `0x00` | iCUE LINK RX MAX Fan | 0 |
| `0x05` | `0x00` | iCUE LINK ADAPTER | 0 |
| `0x05` | `0x01` | 9000D RGB AIRFLOW | 22 |
| `0x05` | `0x02` | 5000T RGB Case | 160 |
| `0x06` | `0x00` | Cooler Pump LCD | 24 |
| `0x07` | `0x00` | H100i RGB | 20 |
| `0x07` | `0x01` | H115i RGB | 20 |
| `0x07` | `0x02` | H150i RGB | 20 |
| `0x07` | `0x03` | H170i RGB | 20 |
| `0x09` | `0x00` | XC7 ELITE CPU Block | 24 |
| `0x0A` | `0x00` | XG3 HYBRID GPU Block | 0 |
| `0x0C` | `0x00` | XD5 ELITE Pump/Res | 22 |
| `0x0D` | `0x00` | XG7 RGB GPU Backplate | 16 |
| `0x0F` | `0x00` | iCUE LINK RX RGB Fan | 8 |
| `0x11` | `0x00` | TITAN 240 AIO | 20 |
| `0x11` | `0x02` | TITAN 360 AIO | 20 |

### 6.9 Keepalive Requirement

The hub reverts to its default rainbow effect if no color packet is received within ~20 seconds. A background keepalive thread must re-send the current color frame every 5 seconds.

### 6.10 Complexity Assessment

Native iCUE LINK is a **high-complexity** driver:
- Hub enumeration with variable-length device records
- Multi-chunk color writes with endpoint lifecycle management
- Keepalive thread to prevent hardware timeout
- LCD framebuffer protocol (separate from LED colors)
- Temperature and fan speed monitoring (read endpoints)
- Software/hardware mode switching with proper cleanup on exit

Estimated effort: 2-3× the Razer driver. Recommended as a standalone implementation milestone.

---

## 7. HAL Integration

### 7.1 Phase 1 — Bridge Backend

```rust
/// Phase 1: OpenLinkHub bridge backend.
///
/// Implements DeviceBackend (not Protocol) — operates at the
/// device management level rather than raw packet encoding.
pub struct CorsairBridgeBackend {
    client: reqwest::Client,
    base_url: String,
    poll_interval: Duration,
    devices: Vec<BridgeDevice>,
}

impl DeviceBackend for CorsairBridgeBackend {
    /* discover, connect, set_color, set_brightness, disconnect */
}
```

### 7.2 Phase 2 — Native Protocol (Future)

```rust
/// Phase 2: Native iCUE LINK protocol driver.
///
/// Implements Protocol trait — pure byte encoding for the
/// 513/512-byte packet format. Transport handles USB HID I/O.
pub struct IcueLinkProtocol {
    /// Enumerated downstream devices with LED counts.
    devices: Vec<LinkDevice>,
    /// Total LED count across all devices.
    total_leds: usize,
}

impl Protocol for IcueLinkProtocol {
    /* encode_color_frame, decode_device_list, etc. */
}
```

### 7.3 Device Family

Requires a new `DeviceFamily::Corsair` variant in `hypercolor-types`:

```rust
pub enum DeviceFamily {
    Wled,
    Hue,
    Razer,
    Corsair,  // new
    Custom(String),
}
```

### 7.4 Protocol Database (Phase 2)

```rust
// Phase 2 registration
corsair_device!(ICUE_LINK_SYSTEM_HUB, 0x0C3F,
    "Corsair iCUE LINK System Hub",
    interface: 0, usage_page: 0xFF42, usage: 0x01);
```

---

## 8. Testing Strategy

### 8.1 Phase 1 — Bridge Backend Tests

**Mock HTTP server:**

A lightweight mock server returns canned OpenLinkHub API responses:

```rust
/// Mock OpenLinkHub API server for testing.
///
/// Serves static JSON fixtures for device discovery,
/// color set, and brightness operations.
pub struct MockOpenLinkHub {
    devices: Vec<serde_json::Value>,
    color_log: Vec<ColorRequest>,
}
```

**Test scenarios:**
- Discovery: `GET /api/devices/` → verify device mapping to `DeviceInfo`
- Color: `POST /api/color` → verify request body format
- Brightness: `POST /api/brightness/gradual` → verify 0-100% scaling
- Health check failure → backend gracefully disabled
- Device disconnect → stale handle detection

**API response fixtures:**
- Captured from a real OpenLinkHub instance with iCUE LINK System Hub
- Covers: QX fans, LCD AIO, temperature probes, fan RPM data

### 8.2 Phase 2 — Native Protocol Tests (Future)

**Packet encoding:**
- Verify 513-byte write buffer layout
- Command byte sequences match reference implementation
- Multi-chunk color data splitting at 508-byte boundaries
- Flat RGB buffer offset calculation across multiple downstream devices

**Device enumeration:**
- Parse variable-length device records
- Handle empty slots (`device_id_length = 0`)
- Device type/model → LED count mapping

**Integration test:**
- Full round-trip: discover → software mode → set color → verify → hardware mode

---

## References

- OpenLinkHub: `github.com/jurkovic-nikola/OpenLinkHub` — REST API documentation
- Local C++ controller references for iCUE LINK, Corsair Peripheral V2, and
  Lighting Node behavior — used as reverse-engineering reference material
