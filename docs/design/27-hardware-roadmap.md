# 27 — Hardware Support Roadmap

## Current State

Hypercolor supports **~160 devices** across 11 driver families spanning USB and network:

### USB Drivers (hypercolor-hal)

| Driver | Devices | Protocol | Transport |
|--------|---------|----------|-----------|
| Razer | 111 | RazerProtocol (Modern/Extended/Extended3F), SeirenV3 | USB HID |
| Corsair | 14 | CorsairLink, LightingNode, CorsairLcd | USB HID |
| ASUS Aura | 10 | AuraUsb (5 controller generations) | USB HidRaw |
| Lian Li | 10 | Ene6k77, TlFan, LegacyUniHub | USB HID |
| QMK | 10 | QmkProtocol (standardized) | USB HID |
| PrismRGB | 4 | PrismRgb | USB HID |
| Dygma | 2 | Dygma (wired/wireless) | USB Serial |
| Ableton Push 2 | 1 | Push2 (MIDI + display) | USB MIDI |

### Network Drivers (hypercolor-driver-*)

| Driver | Protocol | Transport | Features |
|--------|----------|-----------|----------|
| WLED | DDP + E1.31 UDP | Network (HTTP + UDP) | mDNS discovery, segment management |
| Philips Hue | REST + DTLS Entertainment API | Network (HTTP + DTLS) | nUPnP/mDNS discovery, link button pairing, CIE XYb color |
| Nanoleaf | REST + UDP External Control | Network (HTTP + UDP) | mDNS discovery, panel topology, power button pairing |

Plus the ROLI Blocks out-of-process bridge (Spec 30) and Corsair iCUE LINK Phase 2
(native per-LED protocol, Spec 18) in progress.

---

## Market Landscape

The gaming peripherals market is ~$5.1B (2024), growing at ~7% CAGR. RGB is standard in
70%+ of enthusiast builds. The Linux RGB ecosystem is fragmented — OpenRGB covers ~800+
devices but has slowed development, SignalRGB has 1,150+ but is Windows-only, and liquidctl
covers ~40 AIO/fan controller models on Linux.

Hypercolor's opportunity: be the **definitive Linux RGB engine** with real-time effects,
audio reactivity, and a modern Rust codebase that outperforms the C++/Python alternatives.

---

## Vendor Ecosystems Ranked by Priority

### Tier 1: Already Supported — Expand & Solidify

These drivers exist and are production-ready. Focus on expanding device tables and
hardening edge cases.

**Razer** — 111 devices with excellent protocol coverage across keyboards, mice, keypads,
laptops, and microphones. Keep expanding as new hardware ships. Protocol is mature and
well-understood.

**Corsair** — LINK hub, Lighting Node, LCD drivers exist. Phase 2 (native iCUE LINK
protocol for per-LED 60fps) is the big unlock. Commander Core/XT and V2 peripherals are
the next device families to add.

**ASUS Aura** — 10 devices across 5 controller generations (motherboard + addressable +
terminal). Expand motherboard coverage as new chipsets ship.

**Lian Li** — 10 UNI HUB variants. ENE 6K77 and TL/Nuvoton protocols implemented. Watch
for new product lines (SL/AL/TL V3, etc.).

**QMK** — 10 keyboards, but QMK's standardized protocol means adding a new keyboard is
just a device descriptor. Expand the table opportunistically (community PRs welcome).

**WLED** — Full network driver with DDP + E1.31 streaming, mDNS discovery, and segment
management. ~2,100 lines across driver crate and core backend. Massive DIY community.

**Philips Hue** — Full network driver with Entertainment API (DTLS real-time streaming),
nUPnP/mDNS bridge discovery, link button pairing, and CIE XYb color space conversion.
~2,000 lines across driver crate and core backend.

**Nanoleaf** — Full network driver with UDP External Control streaming, mDNS discovery,
panel topology mapping, and power button pairing. ~1,300 lines across driver crate and
core backend.

### Tier 2: High-Impact Additions

These are the biggest gaps in the Linux RGB landscape. Each has existing RE work to
build on, well-understood USB HID protocols, and large user bases.

#### NZXT — Kraken, Smart Device, Hue 2

- **Market position:** Dominant in AIO coolers, popular fan/LED controllers
- **Key devices:** Kraken Elite/Z/X (LCD + pump RGB), Smart Device V2, Hue 2 controllers
- **Protocol:** USB HID, well-documented by liquidctl (Python)
- **RE source:** [liquidctl](https://github.com/liquidctl/liquidctl) — excellent Python
  reference implementations with protocol documentation
- **Effort:** Medium — port liquidctl's protocol knowledge to Rust
- **Impact:** High — fills the biggest AIO gap on Linux

#### Logitech LIGHTSYNC

- **Market position:** ~10-12% peripherals market share, huge installed base
- **Key devices:** G Pro X keyboard, G502/G Pro mice, G733/G935 headsets
- **Protocol:** USB HID, but per-device variants (G203, G810, G915 all differ)
- **RE source:** [g810-led](https://github.com/MatMoul/g810-led),
  [g203-led](https://github.com/smasty/g203-led) — C/Python, covers common models
- **Effort:** Medium-High — many device-specific protocol quirks
- **Impact:** High — Logitech users are underserved on Linux

#### SteelSeries

- **Market position:** Strong in esports/competitive peripherals
- **Key devices:** Apex Pro TKL Gen 3, Aerox mice, Arctis headsets
- **Protocol:** USB HID (GameSense-based commands)
- **RE source:** [steelseriesgg-rs](https://github.com/Ven0m0/steelseriesgg-rs) — **Rust**
  reference implementation, covers keyboards/mice
- **Effort:** Medium — Rust reference makes porting straightforward
- **Impact:** Medium-High — fills esports/competitive niche

### Tier 3: Ecosystem Expansion

Lower urgency but meaningful additions for completeness.

#### Cooler Master

- **Key devices:** MasterFan MF120 Halo, MasterLiquid AIOs, MK750 keyboard
- **Protocol:** USB HID via internal USB controller
- **RE source:** [libcmmk](https://github.com/chmod222/libcmmk) — C library for keyboards;
  OpenRGB covers some fan controllers
- **Effort:** Medium
- **Note:** Cooler Master announced open-sourcing MasterPlus+ but hasn't delivered yet.
  If they do, this drops to Low effort.

#### Thermaltake

- **Key devices:** TOUGHFAN/Riing fans, TT Premium controllers
- **Protocol:** USB HID via 9-pin internal USB controller
- **RE source:** [linux_thermaltake_riing](https://github.com/chestm007/linux_thermaltake_riing)
  — Python daemon with protocol docs
- **Effort:** Medium — straightforward HID protocol

#### Wooting

- **Key devices:** Wooting 80HE, Wooting 60HE — hall-effect keyboard darlings
- **Protocol:** USB HID with official open SDK
- **RE source:** Official SDK — no reverse engineering required
- **Effort:** Low — official documentation + SDK
- **Impact:** Niche but enthusiast-beloved

#### HyperX (HP)

- **Key devices:** Alloy Origins keyboard, Pulsefire mice
- **Protocol:** USB HID
- **RE source:** OpenRGB has partial support
- **Effort:** Medium
- **Note:** NGENUITY software being rebooted in 2025 — protocol may change. Wait for
  stabilization before investing heavily.

#### Govee

- **Key devices:** LED strips, light bars, ambient lighting
- **Protocol:** WiFi + BLE (proprietary), LAN API recently opened
- **Effort:** Medium-High — LAN API is documented but BLE path is not
- **Impact:** Popular ambient lighting brand, complements existing Hue/Nanoleaf/WLED

### Tier 4: Hard Problems

SMBus/I2C devices require kernel module dependencies, have ACPI conflicts, and carry
real hardware risk. Approach with extreme caution.

#### MSI Mystic Light

- **Transport:** SMBus (motherboard RGB controllers)
- **Risk:** **Documented bricking incidents** — OpenRGB partially disabled MSI support
  after reports of boards being bricked
- **Recommendation:** Do NOT implement until MSI provides official documentation or the
  community establishes safe protocols. Monitor
  [msi-mystic-light-x870e](https://github.com/nmelo/msi-mystic-light-x870e) for X870E
  which uses safer USB HID approach.

#### Gigabyte RGB Fusion

- **Transport:** SMBus (motherboard, GPU)
- **Risk:** ACPI conflicts — requires `acpi_enforce_resources=lax` kernel parameter on
  some boards, which weakens system stability
- **Recommendation:** Low priority. Too risky for native drivers.

#### RAM RGB (G.Skill, Kingston, Corsair DDR5)

- **Transport:** SMBus via SPD hub (DDR5 adds SPD5 complexity)
- **Risk:** Writing to SPD can corrupt memory training data
- **Recommendation:** Future consideration only.

#### GPU RGB (cross-vendor)

- **Transport:** I2C via GPU driver, vendor-specific access methods
- **Risk:** Requires GPU driver cooperation, varies by vendor/generation
- **Recommendation:** Out of scope. Too fragmented, too risky.

---

## Protocol Complexity Matrix

| Difficulty | Examples | Transport | Risk |
|-----------|---------|-----------|------|
| **Easy** | QMK, WLED, Nanoleaf, Wooting | USB HID (standard), HTTP/JSON | None |
| **Medium** | NZXT, SteelSeries, Thermaltake, Cooler Master | USB HID (proprietary, well-RE'd) | Low |
| **Medium-Hard** | Logitech, Corsair peripherals, iCUE LINK | USB HID (complex, generation-specific) | Low |
| **Hard** | ASUS Aura mobo, Gigabyte, RAM RGB | SMBus/I2C (kernel deps, ACPI) | Medium |
| **Dangerous** | MSI Mystic Light (some boards) | SMBus | **Bricking risk** |

---

## Existing RE Resources

Key open-source projects to mine for protocol knowledge:

| Project | Language | Covers | Quality |
|---------|----------|--------|---------|
| [liquidctl](https://github.com/liquidctl/liquidctl) | Python | NZXT, Corsair, EVGA | Excellent — clean protocol docs |
| [OpenRGB](https://gitlab.com/CalcProgrammer1/OpenRGB) | C++ | 800+ devices, all vendors | Good but monolithic, varying quality |
| [steelseriesgg-rs](https://github.com/Ven0m0/steelseriesgg-rs) | **Rust** | SteelSeries keyboards/mice | Good — direct Rust reference |
| [g810-led](https://github.com/MatMoul/g810-led) | C++ | Logitech keyboards | Decent — covers common models |
| [libcmmk](https://github.com/chmod222/libcmmk) | C | Cooler Master keyboards | Good — focused scope |
| [linux_thermaltake_riing](https://github.com/chestm007/linux_thermaltake_riing) | Python | Thermaltake fans | Decent |

---

## Recommended Phasing

### Now — Solidify Core
- Expand Razer/Corsair/ASUS/Lian Li device tables as new hardware ships
- Complete Corsair iCUE LINK Phase 2 (native per-LED protocol, Spec 18)
- Accept community QMK device descriptor PRs
- Harden network driver edge cases (WLED/Hue/Nanoleaf reconnection, discovery reliability)

### Next — High-Impact Wave
- **NZXT** driver (port from liquidctl)
- **SteelSeries** driver (port from steelseriesgg-rs)
- **Logitech** driver (port from g810-led, start with keyboards)

### Later — Ecosystem Expansion
- Cooler Master, Thermaltake, Wooting drivers
- Corsair Commander Core/XT family
- HyperX (after NGENUITY protocol stabilizes)
- Govee LAN API

### Not Planned
- MSI Mystic Light, Gigabyte RGB Fusion, RAM RGB, GPU RGB — too risky for native drivers

---

## Success Metrics

| Milestone | Target |
|-----------|--------|
| Tier 1 solidified | Stable across all 11 driver families |
| NZXT shipped | First AIO cooler support on Linux |
| Tier 2 complete | ~250+ devices, covers 80% of enthusiast builds |
| Tier 3 complete | ~350+ devices, competitive with OpenRGB coverage |

---

## Architecture Notes

### USB Drivers (hypercolor-hal)

All USB drivers follow HAL patterns:

- `Protocol` trait for pure byte encoding (no I/O, fully testable)
- `Transport` trait for async USB I/O (9 variants: UsbControl, UsbHidApi, UsbHidRaw,
  UsbHid, UsbBulk, UsbMidi, UsbSerial, I2cSmBus, UsbVendor)
- `ProtocolDatabase` static registry (LazyLock) for device scanning by VID/PID
- `zerocopy` structs for wire-format packets with compile-time size assertions
- `encode_frame_into` for zero-copy frame encoding in the hot path
- `CommandBuffer::push_struct` for allocation-free command building

See: Spec 16 (HAL design).

### Network Drivers (hypercolor-driver-*)

Network drivers use the modular architecture from Spec 35:

- `NetworkDriverFactory` — entry point per driver (discovery, pairing, backend)
- `DiscoveryCapability` — mDNS/nUPnP device discovery
- `PairingCapability` — credential management (link button, power button, API tokens)
- `DriverHost` — daemon-provided credential store and lifecycle hooks
- Shared types in `hypercolor-driver-api` crate

Each network driver has a corresponding backend in `hypercolor-core/src/device/` that
implements `DeviceBackend` for real-time streaming.

See: Spec 33 (network backends), Spec 35 (network driver architecture).
