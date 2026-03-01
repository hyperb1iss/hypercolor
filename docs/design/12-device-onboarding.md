# 12 — Device Onboarding & Discovery

> From install to "lights are working" in under 5 minutes.

**Status:** Design Draft
**Author:** Nova
**Date:** 2026-03-01

---

## Table of Contents

1. [Design Philosophy](#1-design-philosophy)
2. [Auto-Discovery](#2-auto-discovery)
3. [First-Time Setup Wizard](#3-first-time-setup-wizard)
4. [Permission Management](#4-permission-management)
5. [Device Identity](#5-device-identity)
6. [Device Testing](#6-device-testing)
7. [Device Profiles & Presets](#7-device-profiles--presets)
8. [Hot-Plug](#8-hot-plug)
9. [Troubleshooting](#9-troubleshooting)
10. [OpenRGB Coexistence](#10-openrgb-coexistence)
11. [Persona Scenarios](#11-persona-scenarios)

---

## 1. Design Philosophy

Onboarding is make-or-break. Most users will tolerate exactly one failure before going back to Windows and SignalRGB. The entire flow must feel like plugging in a USB stick — the system notices, does the work, and shows you the result.

### Core Principles

- **Zero-config first.** If Hypercolor can figure it out, the user should never have to.
- **Progressive disclosure.** Show the simple path by default; hide advanced options until requested.
- **Fail loudly, recover quietly.** When something breaks, tell the user *what* and *why* and *how to fix it*. Then retry in the background.
- **Confirm with light, not text.** The best proof that a device is working is seeing it light up. Every discovery event should produce a visible flash on the hardware.
- **Respect existing setups.** If OpenRGB is already running, don't fight it — join it.

### Time Budget

| Phase | Target | Maximum |
|---|---|---|
| First device discovered | 10 seconds | 30 seconds |
| All local devices discovered | 30 seconds | 2 minutes |
| Network devices discovered | 45 seconds | 3 minutes |
| Full setup complete | 2 minutes | 5 minutes |
| "Lights are working" | 3 minutes | 5 minutes |

---

## 2. Auto-Discovery

Discovery is a multi-transport sweep that runs at startup, on hot-plug events, and on-demand when the user requests a rescan. Each backend implements the `discover()` method from the `DeviceBackend` trait, but the orchestration layer coordinates them into a unified experience.

### 2.1 Discovery Architecture

```
┌─────────────────────────────────────────────────────┐
│                 DiscoveryOrchestrator                │
│                                                     │
│  Coordinates all backend scanners, deduplicates,    │
│  emits DeviceDiscovered events on the bus           │
│                                                     │
│  ┌─────────────┐ ┌────────────┐ ┌───────────────┐  │
│  │  USB Scanner │ │  mDNS      │ │  OpenRGB SDK  │  │
│  │  (udev +     │ │  Scanner   │ │  Scanner      │  │
│  │   hidapi)    │ │  (mdns-sd) │ │  (TCP 6742)   │  │
│  └──────┬──────┘ └─────┬──────┘ └──────┬────────┘  │
│         │              │               │            │
│  ┌──────┴──────┐ ┌─────┴──────┐ ┌──────┴────────┐  │
│  │  UDP Bcast  │ │  HTTP Scan │ │  Bluetooth    │  │
│  │  Scanner    │ │  (Hue)     │ │  Scanner      │  │
│  │  (WLED)     │ │            │ │  (future)     │  │
│  └─────────────┘ └────────────┘ └───────────────┘  │
│                                                     │
│  ┌─────────────────────────────────────────────┐    │
│  │              Device Registry                 │    │
│  │  Persistent store of known devices,          │    │
│  │  their identities, and connection history    │    │
│  └─────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────┘
```

### 2.2 USB Enumeration (HID Devices)

Covers PrismRGB (Prism 8, Prism S, Prism Mini), Nollie 8, and any future USB HID controllers.

**Mechanism:** `udev` monitor + `hidapi` enumeration.

```rust
/// Known USB HID devices — VID:PID registry
const USB_DEVICE_TABLE: &[(u16, u16, &str, DeviceFamily)] = &[
    (0x16D5, 0x1F01, "PrismRGB Prism 8",   DeviceFamily::Prism8),
    (0x16D2, 0x1F01, "Nollie 8 v2",         DeviceFamily::Nollie8),
    (0x16D0, 0x1294, "PrismRGB Prism S",    DeviceFamily::PrismS),
    (0x16D0, 0x1407, "PrismRGB Prism Mini",  DeviceFamily::PrismMini),
];

pub struct UsbScanner {
    /// udev monitor for hot-plug events
    monitor: UdevMonitor,
    /// VID:PID lookup table
    known_devices: &'static [(u16, u16, &'static str, DeviceFamily)],
}

impl UsbScanner {
    /// Full enumeration — called at startup and on manual rescan
    pub fn enumerate(&self) -> Vec<DiscoveredDevice> {
        let hid_api = HidApi::new().expect("Failed to init hidapi");
        hid_api.device_list()
            .filter_map(|dev| {
                self.known_devices.iter()
                    .find(|(vid, pid, _, _)| dev.vendor_id() == *vid && dev.product_id() == *pid)
                    .map(|(_, _, name, family)| DiscoveredDevice {
                        transport: Transport::UsbHid,
                        name: name.to_string(),
                        family: *family,
                        identifier: DeviceIdentifier::usb(
                            dev.vendor_id(),
                            dev.product_id(),
                            dev.serial_number().map(String::from),
                            dev.path().to_string_lossy().to_string(),
                        ),
                        needs_permissions: !self.check_hid_access(dev.path()),
                    })
            })
            .collect()
    }

    /// Hot-plug listener — runs continuously on a dedicated tokio task
    pub async fn watch(&mut self, tx: broadcast::Sender<DiscoveryEvent>) {
        loop {
            match self.monitor.next_event().await {
                UdevEvent::Add(dev) => {
                    if let Some(discovered) = self.match_device(&dev) {
                        let _ = tx.send(DiscoveryEvent::DeviceAppeared(discovered));
                    }
                }
                UdevEvent::Remove(dev) => {
                    let _ = tx.send(DiscoveryEvent::DeviceVanished(dev.syspath().into()));
                }
            }
        }
    }
}
```

**Discovery flow:**

```
USB cable plugged in
        │
        ▼
  udev fires ADD event
        │
        ▼
  UsbScanner matches VID:PID
  against known device table
        │
        ├─ Match found ──▶ DiscoveryEvent::DeviceAppeared
        │                        │
        │                        ▼
        │                  Check HID permissions
        │                        │
        │                   ┌────┴─────┐
        │                   │          │
        │                 OK ✓     Permission denied
        │                   │          │
        │                   ▼          ▼
        │              Auto-connect   Emit PermissionNeeded
        │              + identify     event (see §4)
        │              flash
        │
        └─ No match ──▶ Ignore (or log for debugging)
```

### 2.3 mDNS / Zeroconf (WLED, Hue Bridge)

Covers WLED devices (service type `_wled._tcp`) and Philips Hue bridges (service type `_hue._tcp`).

**Mechanism:** `mdns-sd` crate for asynchronous mDNS browsing.

```rust
pub struct MdnsScanner {
    daemon: ServiceDaemon,
    wled_browser: ServiceBrowser,
    hue_browser: ServiceBrowser,
}

impl MdnsScanner {
    pub fn new() -> Result<Self> {
        let daemon = ServiceDaemon::new()?;
        let wled_browser = daemon.browse("_wled._tcp.local.")?;
        let hue_browser = daemon.browse("_hue._tcp.local.")?;
        Ok(Self { daemon, wled_browser, hue_browser })
    }

    pub async fn watch(&mut self, tx: broadcast::Sender<DiscoveryEvent>) {
        loop {
            tokio::select! {
                event = self.wled_browser.recv_async() => {
                    if let Ok(ServiceEvent::ServiceResolved(info)) = event {
                        let device = DiscoveredDevice {
                            transport: Transport::WledDdp,
                            name: info.get_hostname().trim_end_matches('.').to_string(),
                            family: DeviceFamily::Wled,
                            identifier: DeviceIdentifier::network(
                                info.get_addresses().iter().next().copied(),
                                info.get_port(),
                                self.extract_mac(&info),
                            ),
                            needs_permissions: false,
                        };
                        let _ = tx.send(DiscoveryEvent::DeviceAppeared(device));
                    }
                }
                event = self.hue_browser.recv_async() => {
                    if let Ok(ServiceEvent::ServiceResolved(info)) = event {
                        let device = DiscoveredDevice {
                            transport: Transport::HueHttp,
                            name: format!("Hue Bridge ({})",
                                info.get_properties().get("bridgeid")
                                    .unwrap_or(&"unknown".to_string())),
                            family: DeviceFamily::PhilipsHue,
                            identifier: DeviceIdentifier::hue_bridge(
                                info.get_properties().get("bridgeid").cloned(),
                                info.get_addresses().iter().next().copied(),
                            ),
                            needs_permissions: false,
                        };
                        let _ = tx.send(DiscoveryEvent::DeviceAppeared(device));
                    }
                }
            }
        }
    }
}
```

**WLED-specific enrichment:** After mDNS discovery, Hypercolor issues a `GET /json/info` request to each WLED device to retrieve LED count, firmware version, MAC address, and segment configuration. This turns a bare IP into a fully characterized device.

```
mDNS announces _wled._tcp
        │
        ▼
  Resolve hostname + IP
        │
        ▼
  GET http://<ip>/json/info
        │
        ▼
  Parse: LED count, firmware,
  MAC, segments, RGBW mode
        │
        ▼
  DiscoveryEvent::DeviceAppeared
  (fully characterized)
```

### 2.4 UDP Broadcast (WLED Fallback)

For WLED devices on the same subnet that might not respond to mDNS (common on some router configurations), a UDP broadcast scan fills in the gaps.

```rust
pub struct UdpBroadcastScanner;

impl UdpBroadcastScanner {
    /// Send a DDP discovery packet to the broadcast address
    /// WLED responds to DDP identify packets on port 4048
    pub async fn scan_subnet(&self, interface_addr: IpAddr) -> Vec<DiscoveredDevice> {
        let broadcast = Self::broadcast_for(interface_addr);
        let socket = UdpSocket::bind("0.0.0.0:0").await.unwrap();
        socket.set_broadcast(true).unwrap();

        // DDP identify packet
        let identify = ddp::IdentifyPacket::new();
        socket.send_to(&identify.to_bytes(), (broadcast, 4048)).await.ok();

        let mut found = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(3);

        loop {
            let timeout = deadline.saturating_duration_since(Instant::now());
            match tokio::time::timeout(timeout, socket.recv_from(&mut [0u8; 1500])).await {
                Ok(Ok((_, addr))) => {
                    // Verify it's actually WLED by hitting the JSON API
                    if let Some(device) = self.verify_wled(addr.ip()).await {
                        found.push(device);
                    }
                }
                _ => break,
            }
        }
        found
    }
}
```

### 2.5 OpenRGB SDK Discovery

If OpenRGB is already running, Hypercolor can connect as a client and import all devices that OpenRGB already manages — motherboard LEDs, GPU, RAM, Corsair, Razer, and everything else OpenRGB supports.

```rust
pub struct OpenRgbScanner {
    /// Default: localhost:6742. Configurable for remote instances.
    endpoint: SocketAddr,
}

impl OpenRgbScanner {
    pub async fn probe(&self) -> Result<Vec<DiscoveredDevice>> {
        let client = OpenRGB::connect_to(self.endpoint).await?;
        let controller_count = client.get_controller_count().await?;

        let mut devices = Vec::with_capacity(controller_count as usize);
        for i in 0..controller_count {
            let ctrl = client.get_controller(i).await?;
            devices.push(DiscoveredDevice {
                transport: Transport::OpenRgbSdk,
                name: ctrl.name.clone(),
                family: DeviceFamily::OpenRgb,
                identifier: DeviceIdentifier::openrgb(
                    ctrl.name.clone(),
                    ctrl.location.clone(),
                    i,
                ),
                needs_permissions: false,
            });
        }
        Ok(devices)
    }
}
```

**Connection strategy:**

```
Daemon starts
    │
    ▼
Attempt TCP connect to localhost:6742
    │
    ├─ Success ──▶ OpenRGB is running
    │               │
    │               ▼
    │         Enumerate controllers
    │         Import as OpenRGB-managed devices
    │         Subscribe to controller updates
    │
    └─ Refused ──▶ OpenRGB not running
                    │
                    ▼
              Use native backends only
              (USB HID, WLED, Hue)
              Optionally offer to start OpenRGB
```

### 2.6 Bluetooth Scanning (Future)

Reserved for Govee, Nanoleaf, and other BLE-controlled LED devices. Architecture placeholder:

```rust
pub struct BluetoothScanner {
    adapter: BtleAdapter,
    /// Known BLE service UUIDs for RGB devices
    target_services: Vec<Uuid>,
}

impl BluetoothScanner {
    pub async fn scan(&self, duration: Duration) -> Vec<DiscoveredDevice> {
        // btleplug crate for cross-platform BLE
        // Filter by known service UUIDs
        // Return characterized devices
        todo!("Phase 4+ — Bluetooth device support")
    }
}
```

### 2.7 Discovery Orchestration

The `DiscoveryOrchestrator` coordinates all scanners, deduplicates results, and emits unified events.

```rust
pub struct DiscoveryOrchestrator {
    usb: UsbScanner,
    mdns: MdnsScanner,
    udp: UdpBroadcastScanner,
    openrgb: OpenRgbScanner,
    registry: DeviceRegistry,
    bus: broadcast::Sender<HypercolorEvent>,
}

impl DiscoveryOrchestrator {
    /// Full discovery sweep — called at startup and on manual rescan
    pub async fn full_scan(&mut self) -> DiscoveryReport {
        let (usb, mdns, udp, openrgb) = tokio::join!(
            self.usb.enumerate_async(),
            self.mdns.collect_current(),
            self.udp.scan_all_interfaces(),
            self.openrgb.probe(),
        );

        let mut all: Vec<DiscoveredDevice> = Vec::new();
        all.extend(usb);
        all.extend(mdns);
        all.extend(udp.unwrap_or_default());
        all.extend(openrgb.unwrap_or_default());

        // Deduplicate: same physical device found by multiple scanners
        let deduped = self.deduplicate(all);

        // Diff against registry: which are new, which reconnected, which gone?
        let report = self.registry.diff(&deduped);

        // Emit events
        for device in &report.new_devices {
            let _ = self.bus.send(HypercolorEvent::DeviceConnected(
                device.to_device_info()
            ));
        }

        // Persist new devices to registry
        self.registry.merge(deduped);

        report
    }

    /// Deduplication: WLED found via both mDNS and UDP broadcast
    fn deduplicate(&self, devices: Vec<DiscoveredDevice>) -> Vec<DiscoveredDevice> {
        let mut seen: HashMap<DeviceFingerprint, DiscoveredDevice> = HashMap::new();
        for dev in devices {
            let fp = dev.fingerprint();
            seen.entry(fp)
                .and_modify(|existing| existing.merge_metadata(&dev))
                .or_insert(dev);
        }
        seen.into_values().collect()
    }
}
```

### 2.8 Discovery Notification

When devices are found, the user sees them immediately — both in the UI and as a physical confirmation.

**Web UI notification:**

```
┌──────────────────────────────────────────┐
│  ✦ Found 3 new devices                   │
│                                          │
│  ◈ PrismRGB Prism 8     USB HID         │
│  ◈ WLED Kitchen Strip   mDNS (WiFi)     │
│  ◈ Hue Bridge           mDNS (HTTP)     │
│                                          │
│  [ Configure Now ]   [ Dismiss ]         │
└──────────────────────────────────────────┘
```

**D-Bus notification (desktop integration):**

```rust
// Emit desktop notification via D-Bus
fn notify_discovery(count: usize, device_names: &[String]) {
    let summary = format!("Hypercolor: Found {} new device{}", count,
        if count == 1 { "" } else { "s" });
    let body = device_names.join("\n");
    // org.freedesktop.Notifications.Notify
    dbus_notify(&summary, &body, "hypercolor-device-found");
}
```

**Physical confirmation:** Each newly discovered device gets a brief cyan flash (the SilkCircuit accent color `#80ffea`) to visually confirm "yes, that device is connected and responsive."

---

## 3. First-Time Setup Wizard

The wizard runs automatically when the daemon starts with zero configured devices. It can also be triggered manually from the Web UI or CLI (`hypercolor setup`).

### 3.1 Wizard Flow

```
┌─────────────────────────────────────────────────────────────┐
│                                                             │
│  ①  Scan     ②  Review     ③  Configure     ④  Test        │
│  ●──────────○───────────○──────────────○──────────────      │
│                                                             │
│                     ⑤  Layout          ⑥  Effect            │
│              ──────○───────────────○──────────────●         │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

### Step 1 — Scan for Devices

**Duration target:** 5-30 seconds

The wizard runs `DiscoveryOrchestrator::full_scan()` and shows real-time progress as each scanner completes.

```
┌──────────────────────────────────────────────────────────┐
│                                                          │
│            ◆  Scanning for devices...                    │
│                                                          │
│   USB HID devices     ████████████████████  Done (3)     │
│   mDNS (WLED/Hue)     ████████████░░░░░░░  Scanning...  │
│   OpenRGB (TCP 6742)   ████████████████████  Connected   │
│   UDP broadcast        ░░░░░░░░░░░░░░░░░░░  Waiting...  │
│                                                          │
│   Found so far: 11 devices                               │
│                                                          │
│              [ Skip Network Scan ]                       │
│                                                          │
└──────────────────────────────────────────────────────────┘
```

Key behaviors:
- USB scan completes nearly instantly (local enumeration).
- mDNS results stream in as they resolve (1-5 seconds).
- OpenRGB probe returns immediately if the daemon is running, fails fast if not.
- UDP broadcast waits 3 seconds for responses, then completes.
- User can skip network scanning and proceed with local devices only.
- If zero devices found, jump directly to the troubleshooting flow (section 9).

### Step 2 — Review Discovered Devices

All discovered devices are shown in a categorized list with iconography indicating transport type and status.

```
┌──────────────────────────────────────────────────────────────────┐
│                                                                  │
│   Found 14 devices across 4 transports                           │
│                                                                  │
│   ── USB HID ──────────────────────────────────────────────      │
│                                                                  │
│   ⬡  PrismRGB Prism 8          8 channels, 1008 LEDs   ● Ready  │
│   ⬡  PrismRGB Prism S #1       ATX + GPU Strimer       ● Ready  │
│   ⬡  PrismRGB Prism S #2       ATX + GPU Strimer       ● Ready  │
│                                                                  │
│   ── OpenRGB (via SDK) ───────────────────────────────────       │
│                                                                  │
│   ⬡  ASUS Z790-A AURA          3 zones, 18 LEDs        ● Ready  │
│   ⬡  G.Skill Trident Z5 #1     1 zone, 10 LEDs         ● Ready  │
│   ⬡  G.Skill Trident Z5 #2     1 zone, 10 LEDs         ● Ready  │
│   ⬡  Razer Huntsman V2         6 zones, 110 LEDs       ● Ready  │
│   ⬡  Razer Basilisk V3         3 zones, 11 LEDs        ● Ready  │
│   ⬡  Razer Seiren V3 Chroma    1 zone, 16 LEDs         ● Ready  │
│                                                                  │
│   ── WLED (WiFi) ─────────────────────────────────────────       │
│                                                                  │
│   ⬡  wled-kitchen              150 LEDs, v0.15.3        ● Ready  │
│   ⬡  wled-desk-backlight       60 LEDs, v0.15.3         ● Ready  │
│                                                                  │
│   ── Needs Attention ─────────────────────────────────────       │
│                                                                  │
│   ⚠  ASUS RTX 4070S GPU        Needs i2c-dev module    ▲ Action  │
│   ⚠  Corsair iCUE LINK Hub     Not detected (OpenRGB)  ▲ Action  │
│                                                                  │
│   [ Add Device Manually ]        [ Continue → ]                  │
│                                                                  │
└──────────────────────────────────────────────────────────────────┘
```

**Status indicators:**

| Icon | Meaning |
|---|---|
| `● Ready` | Device connected and communicating |
| `▲ Action` | Needs user intervention (permissions, driver, config) |
| `○ Offline` | Previously known device not currently reachable |
| `◌ Unknown` | Device found but not yet characterized |

Clicking/tapping a device in the "Needs Attention" section opens the relevant troubleshooting flow inline.

### Step 3 — Configure (Auto vs. Manual)

For most devices, auto-configuration is the default. Hypercolor uses the built-in device database (see section 7) to set correct LED counts, color formats, topologies, and brightness limits.

```
┌──────────────────────────────────────────────────────────────┐
│                                                              │
│   Auto-configuring 12 devices...                             │
│                                                              │
│   ✓ PrismRGB Prism 8       8ch × 126 LEDs, GRB, 0.75 bri   │
│   ✓ PrismRGB Prism S #1    ATX 20×6 + GPU 27×4, RGB         │
│   ✓ PrismRGB Prism S #2    ATX 20×6 + GPU 27×6, RGB         │
│   ✓ ASUS Z790-A AURA       via OpenRGB, 3 zones             │
│   ✓ G.Skill Trident Z5 ×2  via OpenRGB, 10 LEDs each        │
│   ✓ Razer Huntsman V2      via OpenRGB, per-key             │
│   ✓ Razer Basilisk V3      via OpenRGB, 3 zones             │
│   ✓ Razer Seiren V3        via OpenRGB, ring 16 LEDs        │
│   ✓ wled-kitchen           150 LEDs, DDP, RGBW              │
│   ✓ wled-desk-backlight    60 LEDs, DDP, RGB                │
│                                                              │
│   ⚠ 2 devices need manual configuration                     │
│   → ASUS RTX 4070S: [Load i2c-dev module]                   │
│   → Corsair iCUE LINK: [Install OpenLinkHub]                │
│                                                              │
│   [ Review Settings ]          [ Continue → ]                │
│                                                              │
└──────────────────────────────────────────────────────────────┘
```

**Manual override available for:**
- LED count per channel (user attached fewer LEDs than the max)
- Color format override (rare edge case: wrong color order)
- Brightness ceiling (power management for WS2812 strips)
- IP address override for WLED on different VLANs
- Custom zone naming

### Step 4 — Test Each Device

The identity test is the critical confidence-building moment. Each device should visually prove it's connected and controllable.

```
┌──────────────────────────────────────────────────────────┐
│                                                          │
│   Device Test — PrismRGB Prism 8                         │
│                                                          │
│   Channel 1  ████████████████████████  60 LEDs           │
│   Channel 2  ████████████████████████  60 LEDs           │
│   Channel 3  ████████████████████████  42 LEDs           │
│   Channel 4  ████████████████████████  126 LEDs          │
│   Channel 5  (empty)                                     │
│   Channel 6  (empty)                                     │
│   Channel 7  (empty)                                     │
│   Channel 8  (empty)                                     │
│                                                          │
│   [ ◈ Identify All ]  [ ◈ Identify Ch1 ]                │
│                                                          │
│   Color sweep:   Red ✓   Green ✓   Blue ✓   White ✓     │
│   Latency:       0.8ms round-trip                        │
│   LED count:     288 detected (matches config)           │
│                                                          │
│   [ ✓ Looks Good ]    [ ✗ Something's Wrong ]            │
│                                                          │
│   ◁ Prev Device    3 / 12    Next Device ▷               │
│                                                          │
└──────────────────────────────────────────────────────────┘
```

**Test sequence (automatic, ~4 seconds):**

1. **Identify flash** — All LEDs on the device flash cyan (`#80ffea`) three times.
2. **Red sweep** — All LEDs set to `(255, 0, 0)` for 500ms.
3. **Green sweep** — All LEDs set to `(0, 255, 0)` for 500ms.
4. **Blue sweep** — All LEDs set to `(0, 0, 255)` for 500ms.
5. **White sweep** — All LEDs set to `(255, 255, 255)` for 500ms.
6. **Count verification** — Chase pattern runs across all LEDs to verify count.

If the user presses "Something's Wrong" they're routed into the relevant troubleshooting path (section 9).

### Step 5 — Quick Layout (Canvas Placement)

Place devices on the spatial canvas. This is a simplified version of the full layout editor — the wizard variant offers one-click presets and drag-and-drop.

```
┌──────────────────────────────────────────────────────────────────┐
│                                                                  │
│   Place Your Devices                                             │
│                                                                  │
│   ┌──────────────────────────────────────────────────────────┐   │
│   │                      Canvas (320×200)                     │   │
│   │                                                          │   │
│   │    ┌──┐  [ASUS AURA]  ┌──┐                               │   │
│   │    │  │               │  │  ← RAM sticks                 │   │
│   │    └──┘               └──┘                               │   │
│   │                                                          │   │
│   │    ╔═══════════════════════════════╗                      │   │
│   │    ║   Prism S #1 — ATX Strimer   ║ ← drag to position  │   │
│   │    ╚═══════════════════════════════╝                      │   │
│   │                                                          │   │
│   │         ┌───────────┐      ○ ○ ○ ○ ← fans               │   │
│   │         │  GPU      │      ○     ○                       │   │
│   │         │  Strimer  │      ○ ○ ○ ○                       │   │
│   │         └───────────┘                                    │   │
│   │                                                          │   │
│   │    ════════════════════════════  ← desk WLED strip       │   │
│   │                                                          │   │
│   └──────────────────────────────────────────────────────────┘   │
│                                                                  │
│   Quick Presets:                                                  │
│   [ Desktop Tower ]  [ Desk Setup ]  [ Room Layout ]             │
│                                                                  │
│   [ Skip Layout ]            [ Continue → ]                      │
│                                                                  │
└──────────────────────────────────────────────────────────────────┘
```

**Quick presets:**
- **Desktop Tower** — Arranges motherboard, GPU, RAM, fans, and strimers in a typical PC case layout.
- **Desk Setup** — Puts monitor backlight strip at top, desk edge strips at bottom, peripherals in between.
- **Room Layout** — Wide canvas for WLED strips and Hue bulbs across a room.

The layout is persisted in the user's profile and fully editable later via the spatial layout editor.

### Step 6 — Pick an Effect and Go

The final step. Choose an effect, see it render live on all devices.

```
┌──────────────────────────────────────────────────────────────┐
│                                                              │
│   Choose Your First Effect                                   │
│                                                              │
│   ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────┐      │
│   │ ▶        │ │          │ │          │ │          │      │
│   │ Rainbow  │ │  Aurora   │ │  Pulse   │ │  Chill   │      │
│   │  Wave    │ │ Borealis │ │  Fade    │ │  Glow    │      │
│   └──────────┘ └──────────┘ └──────────┘ └──────────┘      │
│                                                              │
│   ┌──────────────────────────────────────────────────────┐   │
│   │                                                      │   │
│   │            [ Live Preview on Canvas ]                 │   │
│   │           All 14 devices rendering live               │   │
│   │                                                      │   │
│   └──────────────────────────────────────────────────────┘   │
│                                                              │
│   ◈ Audio reactive?  [ Enable Microphone ]                   │
│                                                              │
│                          [ ✦ Finish Setup ]                  │
│                                                              │
└──────────────────────────────────────────────────────────────┘
```

**On completion:**
- Save the selected effect as the default profile.
- Save the device configuration + layout to `~/.config/hypercolor/config.toml`.
- The daemon continues running with the chosen effect.
- Desktop notification: "Hypercolor is running — 14 devices, Rainbow Wave."

### 3.2 TUI Wizard Variant

For users running headless or via SSH, the TUI offers an equivalent flow using Ratatui widgets.

```
╔══════════════════════════════════════════════════════════════╗
║  Hypercolor Setup                             Step 2 of 6   ║
╠══════════════════════════════════════════════════════════════╣
║                                                              ║
║  Found 14 devices:                                           ║
║                                                              ║
║  ▸ ● PrismRGB Prism 8          USB HID    1008 LEDs         ║
║    ● PrismRGB Prism S #1       USB HID    282 LEDs          ║
║    ● PrismRGB Prism S #2       USB HID    282 LEDs          ║
║    ● ASUS Z790-A AURA          OpenRGB    18 LEDs           ║
║    ● G.Skill Trident Z5 #1     OpenRGB    10 LEDs           ║
║    ● G.Skill Trident Z5 #2     OpenRGB    10 LEDs           ║
║    ● Razer Huntsman V2         OpenRGB    110 LEDs          ║
║    ● Razer Basilisk V3         OpenRGB    11 LEDs           ║
║    ● Razer Seiren V3 Chroma    OpenRGB    16 LEDs           ║
║    ● wled-kitchen              WLED DDP   150 LEDs          ║
║    ● wled-desk-backlight       WLED DDP   60 LEDs           ║
║    ▲ ASUS RTX 4070S            i2c-dev    needs module      ║
║    ▲ Corsair iCUE LINK         OpenRGB    not detected      ║
║                                                              ║
║  [↑↓] Navigate  [Enter] Details  [I] Identify  [→] Next     ║
╚══════════════════════════════════════════════════════════════╝
```

### 3.3 CLI Quick-Start

For power users who just want it working:

```bash
# Full auto: scan, auto-configure, apply default effect
hypercolor setup --auto

# Scan only, report what's found
hypercolor discover

# Add a specific WLED device by IP
hypercolor device add wled 192.168.1.42 --name "Kitchen Shelf"

# Test a specific device
hypercolor device test "PrismRGB Prism 8"
```

---

## 4. Permission Management

Linux USB device access is the #1 reason onboarding fails. Hypercolor must handle this gracefully.

### 4.1 The Permission Landscape

| Resource | Default Permission | What Hypercolor Needs | Solution |
|---|---|---|---|
| USB HID devices (`/dev/hidraw*`) | root only | Read + Write | udev rule |
| i2c bus (`/dev/i2c-*`) | root only | Read + Write (SMBus RGB) | udev rule + kernel module |
| PipeWire audio capture | User session | Capture monitor source | PipeWire permission grant |
| Network sockets (UDP/TCP) | User | Outbound to WLED/Hue/OpenRGB | No action needed |
| D-Bus session bus | User session | Publish service interface | No action needed |

### 4.2 udev Rules

Hypercolor ships a udev rules file that grants the `plugdev` group access to all known RGB HID devices.

**File:** `resources/udev/99-hypercolor.rules`

```udev
# Hypercolor RGB device access rules
# Install: sudo cp 99-hypercolor.rules /etc/udev/rules.d/
# Reload: sudo udevadm control --reload-rules && sudo udevadm trigger

# PrismRGB Prism S
SUBSYSTEM=="hidraw", ATTRS{idVendor}=="16d0", ATTRS{idProduct}=="1294", MODE="0660", GROUP="plugdev", TAG+="uaccess"
SUBSYSTEM=="usb", ATTRS{idVendor}=="16d0", ATTRS{idProduct}=="1294", MODE="0660", GROUP="plugdev", TAG+="uaccess"

# PrismRGB Prism 8
SUBSYSTEM=="hidraw", ATTRS{idVendor}=="16d5", ATTRS{idProduct}=="1f01", MODE="0660", GROUP="plugdev", TAG+="uaccess"
SUBSYSTEM=="usb", ATTRS{idVendor}=="16d5", ATTRS{idProduct}=="1f01", MODE="0660", GROUP="plugdev", TAG+="uaccess"

# PrismRGB Prism Mini
SUBSYSTEM=="hidraw", ATTRS{idVendor}=="16d0", ATTRS{idProduct}=="1407", MODE="0660", GROUP="plugdev", TAG+="uaccess"
SUBSYSTEM=="usb", ATTRS{idVendor}=="16d0", ATTRS{idProduct}=="1407", MODE="0660", GROUP="plugdev", TAG+="uaccess"

# Nollie 8 v2
SUBSYSTEM=="hidraw", ATTRS{idVendor}=="16d2", ATTRS{idProduct}=="1f01", MODE="0660", GROUP="plugdev", TAG+="uaccess"
SUBSYSTEM=="usb", ATTRS{idVendor}=="16d2", ATTRS{idProduct}=="1f01", MODE="0660", GROUP="plugdev", TAG+="uaccess"

# i2c/SMBus (for motherboard and GPU RGB via OpenRGB)
KERNEL=="i2c-[0-9]*", MODE="0660", GROUP="plugdev", TAG+="uaccess"
```

**TAG+="uaccess"** — This is the modern systemd approach. It grants access to any user with an active login session (seat), regardless of group membership. The `GROUP="plugdev"` is a fallback for systems without systemd-logind.

### 4.3 Auto-Install Flow

When Hypercolor detects that udev rules are missing (a device was found but permission was denied), it offers guided installation.

```
┌──────────────────────────────────────────────────────────────┐
│                                                              │
│  ⚠  Permission Required                                     │
│                                                              │
│  Hypercolor found a PrismRGB Prism 8 but can't access it.   │
│  USB HID devices need a udev rule for non-root access.      │
│                                                              │
│  ── Option A: Automatic (recommended) ─────────────────      │
│                                                              │
│  Run this command (requires sudo):                           │
│                                                              │
│  ┌────────────────────────────────────────────────────┐      │
│  │ sudo hypercolor permissions install                │ 📋   │
│  └────────────────────────────────────────────────────┘      │
│                                                              │
│  This will:                                                  │
│  • Copy udev rules to /etc/udev/rules.d/                    │
│  • Add your user to the plugdev group                        │
│  • Reload udev rules                                         │
│  • No reboot required (re-plug the USB device)               │
│                                                              │
│  ── Option B: Manual ──────────────────────────────────      │
│                                                              │
│  ┌────────────────────────────────────────────────────┐      │
│  │ sudo cp /usr/share/hypercolor/udev/                │      │
│  │     99-hypercolor.rules /etc/udev/rules.d/         │ 📋   │
│  │ sudo udevadm control --reload-rules                │      │
│  │ sudo udevadm trigger                               │      │
│  └────────────────────────────────────────────────────┘      │
│                                                              │
│  After running, unplug and re-plug the device.               │
│                                                              │
│  [ Retry Detection ]        [ Skip This Device ]             │
│                                                              │
└──────────────────────────────────────────────────────────────┘
```

### 4.4 The `hypercolor permissions` Subcommand

A dedicated CLI tool for permission management.

```bash
# Check current permission status for all device types
hypercolor permissions check

# Install all udev rules + add user to groups
sudo hypercolor permissions install

# Install specific rule set only
sudo hypercolor permissions install --usb-hid
sudo hypercolor permissions install --i2c
sudo hypercolor permissions install --audio

# Remove Hypercolor's udev rules
sudo hypercolor permissions uninstall

# Print the rules file to stdout (for manual inspection)
hypercolor permissions show
```

**Output of `hypercolor permissions check`:**

```
Hypercolor Permission Check
───────────────────────────────────────────────
  USB HID access (/dev/hidraw*)      ● OK
  i2c bus access (/dev/i2c-*)        ▲ No udev rule
  PipeWire audio capture             ● OK
  Network (UDP/TCP outbound)         ● OK
  D-Bus session bus                  ● OK
───────────────────────────────────────────────
  User groups: plugdev ● i2c ▲

  Fix i2c access:
    sudo hypercolor permissions install --i2c
```

### 4.5 i2c/SMBus Access

Motherboard and GPU RGB control via OpenRGB often requires the `i2c-dev` kernel module and user-level access to `/dev/i2c-*` devices.

**Detection:**

```rust
fn check_i2c_access() -> I2cStatus {
    // 1. Is the i2c-dev module loaded?
    let module_loaded = Path::new("/sys/module/i2c_dev").exists();

    // 2. Can we access any i2c device?
    let can_access = (0..20)
        .any(|i| File::open(format!("/dev/i2c-{}", i)).is_ok());

    match (module_loaded, can_access) {
        (false, _)    => I2cStatus::ModuleNotLoaded,
        (true, false) => I2cStatus::PermissionDenied,
        (true, true)  => I2cStatus::Ready,
    }
}
```

**Guided fix:**

```
Module not loaded?
  → sudo modprobe i2c-dev
  → echo "i2c-dev" | sudo tee /etc/modules-load.d/i2c-dev.conf

Permission denied?
  → sudo hypercolor permissions install --i2c
  → (installs udev rule for i2c devices)
```

### 4.6 PipeWire / PulseAudio Audio Capture

Audio-reactive effects need to capture the system audio output (monitor source). On modern Linux this uses PipeWire.

**Detection:**

```rust
fn check_audio_capture() -> AudioStatus {
    // Try to enumerate audio capture devices via cpal
    let host = cpal::default_host();
    match host.default_input_device() {
        Some(_) => AudioStatus::Ready,
        None    => AudioStatus::NoInputDevice,
    }
}
```

Most desktop Linux installations grant audio capture to the user session automatically. If it fails:

```
PipeWire: Ensure your user is in the "pipewire" group
  → sudo usermod -aG pipewire $USER

PulseAudio: Load the monitor module
  → pactl load-module module-loopback source=<monitor-source>
```

### 4.7 SELinux / AppArmor Considerations

**SELinux (Fedora/CentOS):**
- Custom SELinux policy module for Hypercolor's USB HID access.
- Ship as an optional `hypercolor-selinux` package.
- The `hypercolor permissions install` command detects SELinux and offers to install the policy.

**AppArmor (Ubuntu/Debian):**
- Ship an AppArmor profile that allows HID device access, network sockets, and PipeWire.
- Default to `complain` mode so it doesn't block anything, with instructions to switch to `enforce`.

### 4.8 Packaging Integration

Distribution packages should handle permissions automatically:

| Package Format | Permission Setup |
|---|---|
| AUR (CachyOS/Arch) | Post-install hook copies udev rules, runs `udevadm trigger` |
| Flatpak | Portal-based USB device access (XDG Portal) — limited HID support, document limitations |
| AppImage | Bundle `hypercolor permissions install` as post-extract step |
| DEB (Ubuntu/Debian) | Postinst script installs udev rules |
| RPM (Fedora) | %post scriptlet installs udev rules + SELinux policy |
| Nix | `services.udev.packages = [ hypercolor ];` in NixOS config |

---

## 5. Device Identity

Every device needs a stable, unique identity that persists across reboots, reconnects, and USB port changes.

### 5.1 Identity Model

```rust
/// Unique identifier for a device, stable across reconnects
#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub enum DeviceIdentifier {
    /// USB HID: VID + PID + serial number (preferred) or USB path (fallback)
    UsbHid {
        vendor_id: u16,
        product_id: u16,
        serial: Option<String>,
        /// Fallback: USB bus/device path for devices without serial numbers
        usb_path: Option<String>,
    },

    /// WLED: MAC address (stable) + IP (may change with DHCP)
    Wled {
        mac_address: String,       // Primary key — stable across reboots
        ip_address: IpAddr,        // Cached for fast reconnect
        mdns_hostname: Option<String>,
    },

    /// Philips Hue: Bridge ID + individual light serial
    Hue {
        bridge_id: String,
        light_id: String,          // Hue API unique ID per light
        light_serial: Option<String>,
    },

    /// OpenRGB: controller name + location (bus type + address)
    OpenRgb {
        name: String,
        location: String,          // e.g., "HID: /dev/hidraw3" or "I2C: /dev/i2c-1, address 0x29"
        controller_index: u32,     // Index in OpenRGB's controller list
    },
}
```

### 5.2 Identity Resolution Strategy

```
Device connects
      │
      ▼
  Extract hardware identifiers
  (VID:PID + serial, MAC, bridge ID, etc.)
      │
      ▼
  Search DeviceRegistry for match
      │
      ├─ Exact match ──▶ Reconnect known device
      │                    Restore user-assigned name,
      │                    layout position, calibration
      │
      ├─ Partial match ──▶ Probable reconnect
      │   (same VID:PID,     (e.g., different USB port)
      │    no serial,         Prompt user if ambiguous
      │    different path)
      │
      └─ No match ──▶ New device
                       Assign temporary name
                       Add to registry
```

### 5.3 Handling Identical Devices

When two or more devices share the same VID:PID and lack serial numbers (common with PrismRGB controllers), Hypercolor must disambiguate.

**Problem:** Two PrismRGB Prism S controllers (`16D0:1294`) plugged into different USB ports. No serial numbers. How do we tell them apart?

**Solution: Physical identification + user assignment.**

```
┌──────────────────────────────────────────────────────────────┐
│                                                              │
│  ⚠ Two identical devices detected                            │
│                                                              │
│  Both devices are PrismRGB Prism S (16D0:1294).              │
│  Let's figure out which is which.                            │
│                                                              │
│  Flashing device on USB port 1-2.3...                        │
│                                                              │
│  Which device is flashing?                                   │
│                                                              │
│  [ Strimer ATX Cable ]   [ Strimer GPU Cable ]               │
│  [ Case Fans ]           [ Something Else ]                  │
│                                                              │
│  (Or type a custom name)                                     │
│                                                              │
└──────────────────────────────────────────────────────────────┘
```

**Identity persistence for serial-less devices:**

```rust
/// For devices without serial numbers, we use USB topology
/// (bus + port path) as a stable identifier.
/// This breaks if the user moves the USB cable to a different port.
fn usb_topology_id(device: &HidDeviceInfo) -> String {
    // e.g., "usb-0000:00:14.0-2.3" from udev DEVPATH
    device.path().to_string_lossy().to_string()
}

/// If topology changes, use the identify-and-confirm flow
/// to let the user re-associate the device.
fn resolve_ambiguous(
    candidates: &[DiscoveredDevice],
    registry: &DeviceRegistry,
) -> Vec<(DiscoveredDevice, Option<RegisteredDevice>)> {
    // Flash each candidate one at a time
    // User confirms which is which
    // Update registry with new topology path
}
```

### 5.4 Registry Persistence

The device registry is stored as a TOML file alongside the main configuration.

**File:** `~/.config/hypercolor/devices.toml`

```toml
[[device]]
id = "prism8-usb-0000:00:14.0-1"
name = "Prism 8 (Main)"
family = "prism_8"
transport = "usb_hid"
vendor_id = 0x16D5
product_id = 0x1F01
usb_path = "usb-0000:00:14.0-1"
channels = 8
leds_per_channel = [60, 60, 42, 126, 0, 0, 0, 0]
color_format = "grb"
brightness_limit = 0.75
last_seen = 2026-03-01T14:30:00Z

[[device]]
id = "wled-kitchen-a4cf1234abcd"
name = "Kitchen Shelf"
family = "wled"
transport = "wled_ddp"
mac_address = "A4:CF:12:34:AB:CD"
ip_address = "192.168.1.42"
mdns_hostname = "wled-kitchen"
led_count = 150
firmware = "0.15.3"
rgbw = false
last_seen = 2026-03-01T14:30:00Z
```

---

## 6. Device Testing

Testing confirms that communication is working end-to-end. It runs during setup (step 4) and is available on-demand from the device manager.

### 6.1 Test Suite

```rust
pub struct DeviceTestSuite {
    device: DeviceHandle,
}

pub struct TestResults {
    pub identify: TestResult,
    pub color_sweep: TestResult,
    pub led_count: TestResult,
    pub latency: LatencyResult,
    pub firmware: Option<FirmwareInfo>,
}

impl DeviceTestSuite {
    /// Run all tests in sequence
    pub async fn run_all(&self) -> TestResults {
        TestResults {
            identify:    self.test_identify().await,
            color_sweep: self.test_color_sweep().await,
            led_count:   self.test_led_count().await,
            latency:     self.test_latency().await,
            firmware:    self.query_firmware().await.ok(),
        }
    }
}
```

### 6.2 Identify Button

The single most important test. Flashes a specific device or zone so the user can confirm physical identity.

```rust
impl DeviceTestSuite {
    /// Flash the device with a distinctive pattern
    pub async fn test_identify(&self) -> TestResult {
        let identify_color = Rgb::new(128, 255, 234); // #80ffea — SilkCircuit neon cyan
        let off = Rgb::new(0, 0, 0);

        for _ in 0..3 {
            self.device.push_solid(identify_color).await?;
            tokio::time::sleep(Duration::from_millis(200)).await;
            self.device.push_solid(off).await?;
            tokio::time::sleep(Duration::from_millis(150)).await;
        }

        TestResult::Pass
    }

    /// Identify a single zone within a multi-zone device
    pub async fn test_identify_zone(&self, zone: &str) -> TestResult {
        // Only the specified zone flashes; others remain off
        let identify_color = Rgb::new(128, 255, 234);
        let off = Rgb::new(0, 0, 0);

        for _ in 0..3 {
            self.device.push_zone_color(zone, identify_color).await?;
            tokio::time::sleep(Duration::from_millis(200)).await;
            self.device.push_zone_color(zone, off).await?;
            tokio::time::sleep(Duration::from_millis(150)).await;
        }

        TestResult::Pass
    }
}
```

### 6.3 Color Sweep

Verifies that all three color channels are working and in the correct order (catches RGB vs. GRB mismatches).

```rust
impl DeviceTestSuite {
    pub async fn test_color_sweep(&self) -> TestResult {
        let colors = [
            ("Red",   Rgb::new(255, 0, 0)),
            ("Green", Rgb::new(0, 255, 0)),
            ("Blue",  Rgb::new(0, 0, 255)),
            ("White", Rgb::new(255, 255, 255)),
        ];

        for (name, color) in &colors {
            self.device.push_solid(*color).await?;
            tokio::time::sleep(Duration::from_millis(500)).await;
        }

        // Return to off state
        self.device.push_solid(Rgb::new(0, 0, 0)).await?;

        TestResult::Pass
    }
}
```

If the user reports "red looks green" — Hypercolor knows the device has a GRB↔RGB format mismatch and can auto-correct.

### 6.4 LED Count Verification

Runs a chase pattern (single lit LED moving from first to last) to visually confirm the LED count matches what the device reports.

```rust
impl DeviceTestSuite {
    pub async fn test_led_count(&self) -> TestResult {
        let total = self.device.total_leds();
        let chase_color = Rgb::new(255, 106, 193); // #ff6ac1 — SilkCircuit coral

        for i in 0..total {
            let mut frame = vec![Rgb::new(0, 0, 0); total];
            frame[i] = chase_color;
            // Light a small trail for visibility
            if i > 0 { frame[i - 1] = Rgb::new(64, 27, 48); }
            if i > 1 { frame[i - 2] = Rgb::new(16, 7, 12); }

            self.device.push_frame(&frame).await?;
            tokio::time::sleep(Duration::from_millis(15)).await; // ~66fps chase
        }

        self.device.push_solid(Rgb::new(0, 0, 0)).await?;

        // If the chase visually "wrapped" or stopped early,
        // the user can report a count mismatch
        TestResult::Pass
    }
}
```

### 6.5 Latency Test

Measures the round-trip time for a frame update. Critical for understanding which devices can handle 60fps and which need a lower update rate.

```rust
impl DeviceTestSuite {
    pub async fn test_latency(&self) -> LatencyResult {
        let mut samples = Vec::with_capacity(100);

        for _ in 0..100 {
            let start = Instant::now();
            self.device.push_solid(Rgb::new(255, 255, 255)).await.ok();
            samples.push(start.elapsed());
        }

        self.device.push_solid(Rgb::new(0, 0, 0)).await.ok();

        samples.sort();
        LatencyResult {
            min: samples[0],
            median: samples[samples.len() / 2],
            p95: samples[(samples.len() as f64 * 0.95) as usize],
            max: *samples.last().unwrap(),
        }
    }
}
```

**Expected latencies:**

| Transport | Typical Latency | 60fps Viable? |
|---|---|---|
| USB HID (PrismRGB) | 0.5-2ms | Yes |
| OpenRGB SDK (TCP local) | 1-5ms | Yes |
| WLED DDP (WiFi) | 2-15ms | Yes (most) |
| WLED DDP (Ethernet) | 1-3ms | Yes |
| Philips Hue (HTTP) | 30-100ms | No (use Entertainment API) |
| Philips Hue (Entertainment) | 5-20ms | Marginal (25fps target) |

### 6.6 Firmware Version Check

For devices that support firmware queries, report the version and flag known compatibility issues.

```rust
impl DeviceTestSuite {
    pub async fn query_firmware(&self) -> Result<FirmwareInfo> {
        match self.device.family() {
            DeviceFamily::Prism8 | DeviceFamily::Nollie8 => {
                // Send 0xFC 0x01 query, read version byte
                let version = self.device.hid_query(&[0xFC, 0x01]).await?;
                Ok(FirmwareInfo {
                    version: format!("v{}", version[2]),
                    known_issues: match version[2] {
                        1 => vec!["No voltage monitoring support"],
                        _ => vec![],
                    },
                    update_available: None,
                })
            }
            DeviceFamily::PrismMini => {
                // Send 0xCC query
                let response = self.device.hid_query(&[0x00, 0x00, 0x00, 0xCC]).await?;
                Ok(FirmwareInfo {
                    version: format!("{}.{}.{}", response[1], response[2], response[3]),
                    known_issues: vec![],
                    update_available: None,
                })
            }
            DeviceFamily::Wled => {
                // HTTP GET /json/info → version field
                let info: WledInfo = self.device.http_get("/json/info").await?;
                Ok(FirmwareInfo {
                    version: info.ver.clone(),
                    known_issues: if info.ver.starts_with("0.14") {
                        vec!["DDP support limited — upgrade to 0.15+"]
                    } else {
                        vec![]
                    },
                    update_available: None,
                })
            }
            _ => Err(Error::UnsupportedOperation("firmware query")),
        }
    }
}
```

---

## 7. Device Profiles & Presets

A built-in device database provides zero-config setup for known hardware. Community contributions extend coverage.

### 7.1 Device Database Structure

```
resources/devices/
├── prismrgb/
│   ├── prism-8.toml        # Prism 8 / Nollie 8
│   ├── prism-s.toml        # Prism S (Strimer)
│   └── prism-mini.toml     # Prism Mini
├── wled/
│   ├── generic.toml        # Default WLED config
│   ├── sk6812-rgbw.toml    # RGBW strip profile
│   └── ws2812b-60m.toml    # 60 LEDs/m WS2812B
├── openrgb/
│   ├── asus-aura.toml      # ASUS motherboard RGB
│   ├── razer-keyboard.toml # Razer per-key
│   └── corsair-icue.toml   # Corsair iCUE devices
└── hue/
    ├── light-strip.toml    # Hue Light Strip Plus
    ├── play-bar.toml       # Hue Play Bar
    └── bulb-e26.toml       # Standard Hue Bulb
```

### 7.2 Profile Format

```toml
# resources/devices/prismrgb/prism-8.toml

[device]
family = "prism_8"
display_name = "PrismRGB Prism 8"
manufacturer = "Nollie / PrismRGB"
vendor_id = 0x16D5
product_id = 0x1F01
hid_interface = 0

[protocol]
color_format = "grb"
max_channels = 8
max_leds_per_channel = 126
max_total_leds = 1008
packet_size = 65
leds_per_packet = 21
frame_commit = 0xFF
brightness_multiplier = 0.75

[defaults]
topology = "strip"
refresh_rate = 60
shutdown_behavior = "hardware_effect"
hardware_effect_color = [255, 50, 0]  # Warm amber

[topology_presets]
# Common configurations users plug into a Prism 8
fan_120mm = { type = "ring", leds = 16, description = "120mm ARGB fan" }
fan_140mm = { type = "ring", leds = 18, description = "140mm ARGB fan" }
strip_30m = { type = "strip", leds = 30, description = "30 LED/m strip (1m)" }
strip_60m = { type = "strip", leds = 60, description = "60 LED/m strip (1m)" }
strip_144m = { type = "strip", leds = 144, description = "144 LED/m strip (1m)" }

[quirks]
# Known firmware-specific behaviors
voltage_monitoring_min_firmware = 2
brightness_cap = 0.75  # Hardware limitation, not a preference
grb_not_rgb = true      # Critical: wrong byte order = wrong colors
```

```toml
# resources/devices/prismrgb/prism-s.toml

[device]
family = "prism_s"
display_name = "PrismRGB Prism S (Strimer)"
manufacturer = "Nollie / PrismRGB / Lian Li"
vendor_id = 0x16D0
product_id = 0x1294
hid_interface = 2

[protocol]
color_format = "rgb"
brightness_multiplier = 0.50
packet_size = 65
frame_commit = "none"  # No latch byte

[subdevices]
# The Prism S controls two Strimer cables

[subdevices.atx_24pin]
display_name = "24-pin ATX Strimer"
topology = "matrix"
width = 20
height = 6
total_leds = 120

[subdevices.gpu_dual_8pin]
display_name = "Dual 8-pin GPU Strimer"
topology = "matrix"
width = 27
height = 4
total_leds = 108

[subdevices.gpu_triple_8pin]
display_name = "Triple 8-pin GPU Strimer"
topology = "matrix"
width = 27
height = 6
total_leds = 162

[quirks]
brightness_cap = 0.50
no_firmware_query = true
no_voltage_monitoring = true
cable_mode_byte = true  # Must send cable_mode in settings packet
```

### 7.3 Community Device Profiles

Users can contribute device profiles by adding TOML files to `~/.config/hypercolor/devices/` or submitting them to the project repository.

```toml
# ~/.config/hypercolor/devices/custom/my-esp32-matrix.toml

[device]
family = "wled"
display_name = "Living Room LED Matrix"
manufacturer = "DIY (ESP32 + WS2812B)"

[defaults]
topology = "matrix"
width = 16
height = 16
total_leds = 256

[calibration]
# My WS2812B strips run warm — compensate
white_balance = [255, 220, 180]
max_brightness = 0.6  # Power supply can't handle full white on 256 LEDs
gamma = 2.2
```

### 7.4 Device Calibration

Per-device color correction ensures consistent appearance across different LED types.

```rust
pub struct DeviceCalibration {
    /// White point correction (compensate for LED color temperature)
    pub white_balance: Rgb,

    /// Maximum brightness (0.0-1.0) — prevents power supply overload
    pub max_brightness: f32,

    /// Gamma correction curve
    pub gamma: f32,

    /// Per-channel brightness correction (for mismatched LED segments)
    pub channel_balance: Option<Vec<f32>>,
}

impl DeviceCalibration {
    /// Apply calibration to a color before sending to hardware
    pub fn apply(&self, color: Rgb) -> Rgb {
        let r = (color.r as f32 / 255.0).powf(self.gamma) * (self.white_balance.r as f32 / 255.0);
        let g = (color.g as f32 / 255.0).powf(self.gamma) * (self.white_balance.g as f32 / 255.0);
        let b = (color.b as f32 / 255.0).powf(self.gamma) * (self.white_balance.b as f32 / 255.0);

        Rgb::new(
            (r * self.max_brightness * 255.0) as u8,
            (g * self.max_brightness * 255.0) as u8,
            (b * self.max_brightness * 255.0) as u8,
        )
    }
}
```

### 7.5 Firmware Updates

For devices that support it (primarily WLED), Hypercolor can check for and apply firmware updates.

```
┌──────────────────────────────────────────────────────────┐
│                                                          │
│  Firmware Update Available                               │
│                                                          │
│  Device: wled-kitchen (Kitchen Shelf)                    │
│  Current: v0.15.3                                        │
│  Available: v0.16.0                                      │
│                                                          │
│  Changes:                                                │
│  • Improved DDP performance                              │
│  • New segment grouping                                  │
│  • Bug fixes for RGBW mode                               │
│                                                          │
│  [ Update Now ]    [ Remind Me Later ]    [ Skip ]       │
│                                                          │
└──────────────────────────────────────────────────────────┘
```

For USB HID devices (PrismRGB), firmware updates are not supported — there is no known update mechanism. Hypercolor reports the version but cannot modify it.

---

## 8. Hot-Plug

Devices appear and disappear. USB cables get yanked. WiFi drops out. The system must handle all of this without crashing or losing state.

### 8.1 Hot-Plug Architecture

```
┌─────────────────────────────────────────────────────────┐
│                  HotPlugManager                          │
│                                                         │
│  Monitors all transports for connect/disconnect events   │
│  Maintains device health status                          │
│  Orchestrates reconnection with backoff                  │
│                                                         │
│  ┌──────────────┐ ┌──────────────┐ ┌──────────────┐    │
│  │ USB Monitor  │ │ Network      │ │ OpenRGB      │    │
│  │ (udev)       │ │ Monitor      │ │ Monitor      │    │
│  │              │ │ (mDNS +      │ │ (controller  │    │
│  │ ADD/REMOVE   │ │  keepalive)  │ │  updates)    │    │
│  └──────┬───────┘ └──────┬───────┘ └──────┬───────┘    │
│         │                │                │             │
│         └────────┬───────┴────────────────┘             │
│                  │                                       │
│         ┌────────▼────────┐                             │
│         │  State Machine  │                             │
│         │  per device     │                             │
│         └─────────────────┘                             │
│                                                         │
└─────────────────────────────────────────────────────────┘
```

### 8.2 Device State Machine

```
                         ┌─────────┐
              discover   │         │  hot-plug remove
         ┌──────────────▶│  Known  │◀──────────────────┐
         │               │         │                    │
         │               └────┬────┘                    │
         │                    │ connect                  │
         │                    ▼                          │
         │             ┌─────────────┐                  │
         │             │             │                  │
         │             │  Connected  │──────────────┐   │
         │             │             │  comm error   │   │
         │             └──────┬──────┘              │   │
         │                    │                     ▼   │
         │                    │              ┌──────────┴──┐
         │          push_frame│              │             │
         │           success  │              │  Reconnecting│
         │                    │              │  (backoff)   │
         │                    ▼              │             │
         │             ┌─────────────┐      └──────┬──────┘
         │             │             │             │
         │             │   Active    │◀────────────┘
         │             │ (rendering) │  reconnect success
         │             │             │
         │             └──────┬──────┘
         │                    │ user disable
         │                    ▼
         │             ┌─────────────┐
         │             │             │
         └─────────────│  Disabled   │
                       │  (by user)  │
                       │             │
                       └─────────────┘
```

### 8.3 USB Hot-Plug Detection

```rust
pub struct UsbHotPlugMonitor {
    udev_monitor: tokio_udev::MonitorSocket,
}

impl UsbHotPlugMonitor {
    pub async fn run(&mut self, bus: broadcast::Sender<HypercolorEvent>) {
        while let Some(event) = self.udev_monitor.next().await {
            match event.event_type() {
                EventType::Add => {
                    if let Some(device) = self.match_rgb_device(&event) {
                        let _ = bus.send(HypercolorEvent::DeviceConnected(
                            device.to_device_info()
                        ));
                    }
                }
                EventType::Remove => {
                    if let Some(id) = self.match_known_device(&event) {
                        let _ = bus.send(HypercolorEvent::DeviceDisconnected(id));
                    }
                }
                _ => {} // Ignore bind/unbind/change
            }
        }
    }
}
```

### 8.4 Network Device Health Monitoring

WLED and Hue devices can silently drop off WiFi. Periodic health checks detect this.

```rust
pub struct NetworkHealthMonitor {
    /// Known network devices and their last successful contact
    devices: HashMap<DeviceIdentifier, DeviceHealth>,
    /// How often to check (default: every 10 seconds)
    interval: Duration,
}

pub struct DeviceHealth {
    pub last_success: Instant,
    pub consecutive_failures: u32,
    pub state: DeviceState,
}

impl NetworkHealthMonitor {
    pub async fn run(&mut self, bus: broadcast::Sender<HypercolorEvent>) {
        let mut interval = tokio::time::interval(self.interval);

        loop {
            interval.tick().await;

            for (id, health) in &mut self.devices {
                if health.state != DeviceState::Active { continue; }

                match self.ping(id).await {
                    Ok(_) => {
                        health.last_success = Instant::now();
                        health.consecutive_failures = 0;
                    }
                    Err(_) => {
                        health.consecutive_failures += 1;

                        if health.consecutive_failures >= 3 {
                            health.state = DeviceState::Reconnecting;
                            let _ = bus.send(HypercolorEvent::DeviceDisconnected(
                                id.to_string()
                            ));
                        }
                    }
                }
            }
        }
    }

    async fn ping(&self, id: &DeviceIdentifier) -> Result<()> {
        match id {
            DeviceIdentifier::Wled { ip_address, .. } => {
                // Quick HTTP GET /json/state — lightweight health check
                let url = format!("http://{}/json/state", ip_address);
                reqwest::get(&url).await?.error_for_status()?;
                Ok(())
            }
            DeviceIdentifier::Hue { .. } => {
                // HEAD request to bridge
                todo!()
            }
            _ => Ok(()),
        }
    }
}
```

### 8.5 Reconnection with Exponential Backoff

```rust
pub struct ReconnectPolicy {
    pub initial_delay: Duration,     // 1 second
    pub max_delay: Duration,         // 60 seconds
    pub backoff_factor: f64,         // 2.0
    pub max_attempts: Option<u32>,   // None = infinite
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self {
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(60),
            backoff_factor: 2.0,
            max_attempts: None,
        }
    }
}

pub async fn reconnect_loop(
    device: &DiscoveredDevice,
    policy: &ReconnectPolicy,
    bus: broadcast::Sender<HypercolorEvent>,
) -> Result<DeviceHandle> {
    let mut delay = policy.initial_delay;
    let mut attempts = 0u32;

    loop {
        attempts += 1;

        match connect_device(device).await {
            Ok(handle) => {
                let _ = bus.send(HypercolorEvent::DeviceConnected(
                    device.to_device_info()
                ));
                return Ok(handle);
            }
            Err(e) => {
                if policy.max_attempts.is_some_and(|max| attempts >= max) {
                    return Err(e);
                }

                tracing::warn!(
                    device = %device.name,
                    attempt = attempts,
                    next_retry = ?delay,
                    "Reconnect failed: {}", e
                );

                tokio::time::sleep(delay).await;
                delay = Duration::from_secs_f64(
                    (delay.as_secs_f64() * policy.backoff_factor)
                        .min(policy.max_delay.as_secs_f64())
                );
            }
        }
    }
}
```

### 8.6 Graceful Frame Drops

When a device disconnects mid-render, the render loop must not crash. The backend returns an error, and the frame is silently dropped for that device.

```rust
// In the render loop (from ARCHITECTURE.md)
for backend in &mut self.backends {
    match backend.push_frame(&led_colors).await {
        Ok(()) => {}
        Err(DeviceError::Disconnected(id)) => {
            // Device is gone — mark as reconnecting, don't crash
            tracing::warn!(device = %id, "Device disconnected during frame push");
            self.hot_plug.mark_disconnected(&id);
        }
        Err(DeviceError::Timeout(id)) => {
            // Slow device — skip this frame, try again next cycle
            tracing::debug!(device = %id, "Frame push timed out, skipping");
        }
        Err(e) => {
            // Unexpected error — log but continue
            tracing::error!("Backend error: {}", e);
        }
    }
}
```

### 8.7 Notification Behavior

| Event | Web UI | Desktop (D-Bus) | TUI |
|---|---|---|---|
| Device connected | Toast: "PrismRGB Prism 8 connected" | Notification bubble | Status bar update |
| Device disconnected | Toast: "WLED Kitchen offline" | Notification bubble | Status bar update |
| Reconnecting | Spinner on device card | Silent | Blinking indicator |
| Reconnected | Toast: "WLED Kitchen back online" | Notification bubble | Status bar update |
| New device (first time) | Modal: "New device found!" | Notification with action | Prompt |

---

## 9. Troubleshooting

When devices don't work, Hypercolor provides structured diagnostic flows rather than leaving users to grep through logs.

### 9.1 Diagnostic Framework

```rust
pub struct DiagnosticRunner {
    checks: Vec<Box<dyn DiagnosticCheck>>,
}

pub trait DiagnosticCheck: Send + Sync {
    fn name(&self) -> &str;
    fn category(&self) -> DiagCategory;
    fn run(&self) -> DiagResult;
    fn fix_suggestion(&self) -> Option<String>;
}

pub enum DiagResult {
    Pass(String),
    Warn(String),
    Fail(String),
}

pub enum DiagCategory {
    Permissions,
    Hardware,
    Network,
    Protocol,
    Configuration,
}
```

### 9.2 "Device Not Detected" Checklist

When zero devices are found, or a specific device the user expects isn't showing up.

```
hypercolor diagnose

Hypercolor Diagnostics
══════════════════════════════════════════════════════

USB HID Devices
────────────────────────────────────────────────────
  ✓ hidapi library loaded
  ✓ /dev/hidraw* devices present (5 found)
  ▲ Permission check:
      /dev/hidraw0  ✓ readable
      /dev/hidraw1  ✓ readable
      /dev/hidraw2  ✗ permission denied
      /dev/hidraw3  ✓ readable
      /dev/hidraw4  ✗ permission denied
  → Fix: sudo hypercolor permissions install

  VID:PID scan:
      16D5:1F01  ✓ PrismRGB Prism 8 (hidraw0)
      16D0:1294  ✗ PrismRGB Prism S (hidraw2, permission denied)
      16D0:1294  ✗ PrismRGB Prism S (hidraw4, permission denied)

i2c / SMBus
────────────────────────────────────────────────────
  ✗ i2c-dev module not loaded
  → Fix: sudo modprobe i2c-dev

Network Devices
────────────────────────────────────────────────────
  ✓ mDNS browser active
  ✓ Found 2 WLED devices via mDNS
  ✓ UDP broadcast: 2 responses on 192.168.1.0/24

OpenRGB
────────────────────────────────────────────────────
  ✓ OpenRGB daemon running on localhost:6742
  ✓ Protocol version: v5
  ✓ 6 controllers available

Summary: 2 issues found
  1. USB HID permissions needed for 2 devices
  2. i2c-dev kernel module not loaded
```

### 9.3 "Device Detected But No LEDs"

The device communicates but nothing lights up.

```
┌──────────────────────────────────────────────────────────────┐
│                                                              │
│  Protocol Test Mode — PrismRGB Prism 8                       │
│                                                              │
│  Step 1: Raw HID communication                               │
│  ✓ Device opens successfully                                 │
│  ✓ Firmware query (0xFC 0x01) → v2 response                 │
│  ✓ Channel query (0xFC 0x03) → 4 active channels            │
│                                                              │
│  Step 2: Single LED test                                     │
│  Sending GRB (0, 255, 0) to channel 0, LED 0...             │
│  ▲ No visual confirmation from user                          │
│                                                              │
│  Possible causes:                                            │
│  • Wrong HID interface selected (try interface 0)            │
│  • LED strip not connected to channel 0                      │
│  • Power supply not connected to controller                  │
│  • Brightness multiplier too low                             │
│                                                              │
│  [ Try Interface 0 ]  [ Try Full Brightness ]                │
│  [ Skip to Channel 1 ] [ Send Raw Packet ]                   │
│                                                              │
└──────────────────────────────────────────────────────────────┘
```

### 9.4 Intermittent Disconnects — Stability Monitor

For flaky connections, Hypercolor offers a monitoring mode that tracks disconnect events over time and identifies patterns.

```rust
pub struct StabilityMonitor {
    events: Vec<StabilityEvent>,
    window: Duration,  // Default: 1 hour
}

pub struct StabilityEvent {
    device_id: DeviceIdentifier,
    event: StabilityEventType,
    timestamp: SystemTime,
}

pub enum StabilityEventType {
    Connected,
    Disconnected { reason: String },
    FrameDropped,
    LatencySpike { ms: f64 },
}

impl StabilityMonitor {
    pub fn report(&self, device_id: &DeviceIdentifier) -> StabilityReport {
        let events = self.events_for(device_id, self.window);
        StabilityReport {
            disconnects: events.iter().filter(|e| matches!(e.event, StabilityEventType::Disconnected { .. })).count(),
            frame_drops: events.iter().filter(|e| matches!(e.event, StabilityEventType::FrameDropped)).count(),
            avg_latency: self.avg_latency(device_id),
            uptime_percent: self.uptime_percent(device_id),
            pattern: self.detect_pattern(device_id),
        }
    }

    fn detect_pattern(&self, device_id: &DeviceIdentifier) -> Option<String> {
        // Detect periodic disconnects (e.g., every 5 minutes = USB suspend)
        // Detect disconnects correlated with high frame rate
        // Detect disconnects correlated with specific effect types
        todo!()
    }
}
```

**CLI output:**

```
hypercolor device stability "PrismRGB Prism 8"

Stability Report — PrismRGB Prism 8 (last 1 hour)
──────────────────────────────────────────────────────
  Uptime:          98.7%
  Disconnects:     2
  Frame drops:     14
  Avg latency:     1.2ms (p95: 2.8ms)

  Pattern detected: USB autosuspend
  → Both disconnects occurred exactly at the USB autosuspend
    timeout (2 minutes of idle). Disable USB autosuspend:
    echo -1 | sudo tee /sys/bus/usb/devices/1-2.3/power/autosuspend
```

### 9.5 Wrong LED Count — Manual Override

When the device reports a different LED count than what's physically connected.

```
┌──────────────────────────────────────────────────────────────┐
│                                                              │
│  LED Count Override — PrismRGB Prism 8, Channel 3            │
│                                                              │
│  Reported by device: 126 LEDs                                │
│  Current setting:    126 LEDs                                │
│                                                              │
│  The chase test showed LEDs stopping at position ~42.         │
│  This usually means fewer LEDs are connected than the         │
│  controller's maximum.                                       │
│                                                              │
│  New LED count: [ 42 ]                                       │
│                                                              │
│  [ Run Chase Test Again ]     [ Save ]                       │
│                                                              │
└──────────────────────────────────────────────────────────────┘
```

### 9.6 "Everything Was Working Yesterday"

A change detection system that tracks what changed between sessions.

```rust
pub struct ChangeTracker {
    /// Snapshot of system state at last successful run
    last_snapshot: SystemSnapshot,
}

pub struct SystemSnapshot {
    pub timestamp: SystemTime,
    pub kernel_version: String,
    pub usb_devices: Vec<UsbDeviceSnapshot>,
    pub udev_rules_hash: String,
    pub openrgb_version: Option<String>,
    pub hypercolor_config_hash: String,
    pub loaded_modules: Vec<String>,
}

impl ChangeTracker {
    pub fn diff(&self, current: &SystemSnapshot) -> Vec<Change> {
        let mut changes = Vec::new();

        if self.last_snapshot.kernel_version != current.kernel_version {
            changes.push(Change::KernelUpdated {
                from: self.last_snapshot.kernel_version.clone(),
                to: current.kernel_version.clone(),
            });
        }

        // Diff USB devices
        for old_dev in &self.last_snapshot.usb_devices {
            if !current.usb_devices.contains(old_dev) {
                changes.push(Change::UsbDeviceMissing(old_dev.clone()));
            }
        }

        // Check if udev rules changed
        if self.last_snapshot.udev_rules_hash != current.udev_rules_hash {
            changes.push(Change::UdevRulesModified);
        }

        // Check if i2c-dev module is still loaded
        if self.last_snapshot.loaded_modules.contains(&"i2c_dev".to_string())
            && !current.loaded_modules.contains(&"i2c_dev".to_string())
        {
            changes.push(Change::ModuleUnloaded("i2c_dev".to_string()));
        }

        changes
    }
}
```

**CLI output:**

```
hypercolor diagnose --what-changed

What Changed Since Last Successful Run (2026-02-28 22:14)
─────────────────────────────────────────────────────────────
  ▲ Kernel updated: 6.12.8 → 6.12.9
    udev rules may need reloading after kernel update
    → sudo udevadm control --reload-rules && sudo udevadm trigger

  ✗ USB device missing: PrismRGB Prism S (16D0:1294)
    Was on USB port 1-2.3, no longer detected
    → Check USB cable connection

  ✓ OpenRGB version unchanged (1.0rc2)
  ✓ Hypercolor config unchanged
  ✓ i2c-dev module still loaded
```

---

## 10. OpenRGB Coexistence

Hypercolor and OpenRGB are complementary, not competing. The coexistence strategy depends on what's already running.

### 10.1 Detection Matrix

```
Daemon starts
      │
      ▼
  Probe TCP localhost:6742
      │
      ├─ Connected ──▶ OpenRGB daemon is running
      │                   │
      │                   ▼
      │             ┌────────────────────────────────┐
      │             │  OpenRGB Client Mode            │
      │             │                                │
      │             │  Import all controllers         │
      │             │  Use OpenRGB as a "backend"     │
      │             │  Don't touch USB directly for   │
      │             │  devices OpenRGB manages         │
      │             └────────────────────────────────┘
      │
      └─ Refused ──▶ OpenRGB not running
                       │
                       ▼
                 ┌────────────────────────────────┐
                 │  Native Mode                    │
                 │                                │
                 │  USB HID → direct control       │
                 │  WLED → DDP direct              │
                 │  Hue → HTTP direct              │
                 │  i2c → direct (if accessible)   │
                 └────────────────────────────────┘
```

### 10.2 Hybrid Mode

The most common scenario: OpenRGB manages motherboard/GPU/RAM via i2c/SMBus, while Hypercolor directly controls PrismRGB (USB HID) and WLED (DDP). Both systems run simultaneously without conflict.

```rust
pub struct HybridBackendRouter {
    /// Devices managed by OpenRGB — hands off
    openrgb_managed: HashSet<DeviceFingerprint>,
    /// Devices Hypercolor controls directly
    native_managed: HashSet<DeviceFingerprint>,
}

impl HybridBackendRouter {
    pub fn route(&self, device: &DeviceInfo) -> BackendRoute {
        // PrismRGB devices: always native (OpenRGB doesn't support them)
        if matches!(device.family, DeviceFamily::Prism8 | DeviceFamily::PrismS
                                   | DeviceFamily::PrismMini | DeviceFamily::Nollie8) {
            return BackendRoute::Native;
        }

        // WLED: prefer native DDP (better performance than OpenRGB's E1.31)
        if matches!(device.family, DeviceFamily::Wled) {
            return BackendRoute::Native;
        }

        // Everything else: prefer OpenRGB if available
        if self.openrgb_managed.contains(&device.fingerprint()) {
            return BackendRoute::OpenRgb;
        }

        BackendRoute::Native
    }
}
```

### 10.3 Device Conflict Resolution

When both Hypercolor and OpenRGB try to control the same USB device, chaos ensues (garbled colors, flickering). The conflict resolver prevents this.

```
┌──────────────────────────────────────────────────────────────┐
│                                                              │
│  ⚠ Device Conflict Detected                                 │
│                                                              │
│  "Razer Huntsman V2" is controlled by both OpenRGB           │
│  and Hypercolor's native backend.                            │
│                                                              │
│  This causes flickering because both are sending             │
│  different color data simultaneously.                        │
│                                                              │
│  Choose who controls this device:                            │
│                                                              │
│  [ Use OpenRGB ]   Hypercolor sends colors via OpenRGB SDK.  │
│                    Works with all effects. Slightly higher    │
│                    latency (~5ms vs ~1ms).                    │
│                                                              │
│  [ Use Native ]    Hypercolor controls directly via USB HID. │
│                    Lower latency, but OpenRGB loses control.  │
│                    (OpenRGB will show "device busy")          │
│                                                              │
│  [ Disable ]       Neither controls it. Device uses its       │
│                    built-in hardware effect.                  │
│                                                              │
└──────────────────────────────────────────────────────────────┘
```

### 10.4 Feature Comparison Guide

Shown in the UI to help users understand when to use which path.

| Capability | OpenRGB Backend | Native Backend |
|---|---|---|
| Motherboard RGB (ASUS AURA, MSI, etc.) | Excellent | Not supported |
| GPU RGB (SMBus) | Excellent | Not supported |
| RAM RGB (DDR4/DDR5) | Excellent | Not supported |
| Razer peripherals | Good | Not supported |
| Corsair peripherals | Good (via OpenRGB) | Not supported |
| **PrismRGB / Nollie** | **Not supported** | **Excellent** |
| WLED (DDP) | Via E1.31 (170/universe) | **DDP (480/packet)** |
| Philips Hue | Not supported | Good |
| Latency | 1-5ms (SDK overhead) | 0.5-2ms (direct) |
| Setup complexity | Requires OpenRGB daemon | Zero-config for supported devices |

**Recommendation surfaced in the UI:**
> Use OpenRGB for motherboard, GPU, RAM, and brand-name peripherals.
> Use Hypercolor native for PrismRGB controllers and WLED.
> Both work together seamlessly.

### 10.5 Starting OpenRGB from Hypercolor

If OpenRGB is installed but not running, Hypercolor can offer to start it.

```rust
pub async fn ensure_openrgb(config: &OpenRgbConfig) -> Result<OpenRgbConnection> {
    // Try to connect first
    match OpenRGB::connect_to(config.endpoint).await {
        Ok(client) => return Ok(client),
        Err(_) => {
            // Not running — check if installed
            if which::which("openrgb").is_ok() {
                tracing::info!("OpenRGB not running but installed, offering to start");
                // Don't auto-start (Sacred Boundaries) — emit event for UI to prompt
                return Err(Error::OpenRgbNotRunning { installed: true });
            } else {
                return Err(Error::OpenRgbNotRunning { installed: false });
            }
        }
    }
}
```

The UI shows:

```
OpenRGB is installed but not running.
Starting it would add support for 6 more devices
(motherboard, GPU, RAM, Razer peripherals).

[ Start OpenRGB ]    [ Skip ]
```

Hypercolor never starts OpenRGB without explicit user action — that's a sacred boundary.

---

## 11. Persona Scenarios

Three real-world walkthroughs demonstrating the complete onboarding experience.

### Scenario A — Bliss (Power User, 12+ Devices)

> Plugs in 12 devices. Hypercolor auto-discovers 9 via OpenRGB, 2 PrismRGB via USB HID, and 8 WLED via mDNS. Full setup in 10 minutes.

**Timeline:**

```
T+0:00    Install Hypercolor (pacman -S hypercolor)
          → udev rules installed by package post-install hook
          → systemd user service enabled

T+0:30    Open web UI at localhost:9420
          → First-time wizard launches automatically

T+0:35    Step 1 — Scan
          → USB HID: Found Prism 8, Prism S #1, Prism S #2 (3 devices)
          → OpenRGB: Connected to localhost:6742
            → ASUS Z790-A, RTX 4070S, G.Skill ×2, Razer ×3 (7 devices)
          → Corsair iCUE LINK: detected via OpenRGB + OpenLinkHub (2 devices)
          → mDNS: 8 WLED devices resolving...
          → UDP broadcast: confirming on 192.168.1.0/24...
          → Total: 20 devices in 15 seconds

T+0:50    Step 2 — Review
          → All 20 devices shown with correct names and LED counts
          → ASUS GPU flagged: "i2c-dev module loaded, access OK"
          → No permission issues (udev rules from package install)

T+1:00    Step 3 — Configure
          → Auto-config for all 20 devices
          → Bliss manually names the Prism S controllers:
            #1 = "Front Case Strimers", #2 = "Rear Panel Strimers"
          → LED counts from device queries match expected values
          → WLED devices named from their mDNS hostnames

T+2:00    Step 4 — Test
          → Quick identity flash on all devices (3 seconds each)
          → Color sweep confirms all channels working
          → Prism 8 channels 5-8 empty (no strips connected) — skipped
          → Latency: all under 5ms except Hue bulbs (80ms)

T+4:00    Step 5 — Layout
          → Selects "Desktop Tower" preset for PC components
          → Drags WLED devices into approximate room positions
          → Positions Strimers overlaying the case diagram

T+5:00    Step 6 — Effect
          → Selects "Aurora Borealis" from built-in effects
          → All 20 devices light up in coordinated green/purple waves
          → Enables audio reactivity, plays a test track
          → Saves as default profile

T+5:30    Done. 20 devices, zero permission errors, one profile.
```

### Scenario B — Jake (Newcomer, Permission Hurdles)

> Installs Hypercolor, it finds his RGB motherboard via i2c. He needs to add a udev rule. The wizard guides him through it.

**Timeline:**

```
T+0:00    Install Hypercolor via AppImage (download, chmod +x, run)
          → No package manager = no automatic udev rules

T+0:15    Launch hypercolor daemon
          → Web UI opens at localhost:9420
          → First-time wizard starts

T+0:20    Step 1 — Scan
          → USB HID: Found 1 unknown device (Razer keyboard) — permission denied
          → OpenRGB: Connection refused (not installed)
          → mDNS: Nothing found (no WLED devices)
          → Total: 1 device with issues, 0 ready

T+0:30    Step 2 — Review
          → Shows Razer keyboard with ▲ Action indicator
          → "Needs Attention" section expanded by default
          → Jake clicks on the Razer keyboard entry

T+0:35    Permission wizard opens:
          "Hypercolor found a Razer Huntsman V2 but can't access it.
           USB HID devices need a udev rule for non-root access."

          → Jake copies: sudo hypercolor permissions install
          → Pastes into terminal, enters password
          → Output: "✓ udev rules installed, ✓ user added to plugdev"
          → "Unplug and replug your USB device, or run: sudo udevadm trigger"

T+1:00    Jake unplugs and replugs the keyboard
          → Hot-plug detection fires
          → Permission check passes
          → "Razer Huntsman V2 connected!"
          → Keyboard lights up with cyan identify flash

T+1:30    Wizard suggests: "Install OpenRGB for motherboard/GPU RGB support"
          → Jake opens a new tab, installs OpenRGB from AUR
          → Starts openrgb --server
          → Back in Hypercolor, clicks "Rescan"
          → OpenRGB connects: ASUS Z790-A AURA found (3 zones, 18 LEDs)

T+2:00    i2c issue detected:
          "Your ASUS motherboard RGB uses i2c/SMBus. The i2c-dev module
           isn't loaded."
          → sudo modprobe i2c-dev
          → Copy the persistence command too:
            echo "i2c-dev" | sudo tee /etc/modules-load.d/i2c-dev.conf

T+2:30    Rescan again: GPU RGB now detected via i2c
          → 3 devices total: Razer keyboard + ASUS mobo + ASUS GPU

T+3:00    Quick test, skip layout (only 3 devices), pick Rainbow Wave
          → Everything working

T+3:30    Done. 3 devices, 2 permission hurdles overcome with copy-paste commands.
```

### Scenario C — Alex (Network Enthusiast, 30 WLED Devices)

> Has 30 WLED devices across the house. mDNS discovers them all. He names each one. Some are on a different VLAN and need manual IP entry.

**Timeline:**

```
T+0:00    Install Hypercolor (already has it running, adding WLED support)

T+0:10    Scan for devices
          → mDNS: 22 WLED devices found on 192.168.1.0/24
          → UDP broadcast confirms all 22
          → 8 more WLED devices expected on IoT VLAN (192.168.10.0/24)
            → mDNS doesn't cross VLANs by default
            → UDP broadcast can't reach them either

T+0:40    Alex notices 8 missing devices
          → Clicks "Add Device Manually"
          → Enters IP range: 192.168.10.0/24
          → Hypercolor scans the range for WLED JSON API
          → 8 more devices found

T+1:00    All 30 devices listed. Naming time.
          → Alex renames each device with room/location:
            wled-ABCD12 → "Kitchen Shelf"
            wled-EF3456 → "Bedroom Ceiling"
            wled-789ABC → "Hallway Runner"
            ... (28 more)
          → Names persist in devices.toml, keyed by MAC address

T+3:00    Some devices have different LED counts:
          → Kitchen: 150 LEDs (WS2812B 60/m, 2.5m)
          → Bedroom Ceiling: 300 LEDs (WS2812B 60/m, 5m)
          → Garage Strip: 60 LEDs (SK6812 RGBW 30/m, 2m)
          → Alex confirms each via the LED count chase test

T+4:00    Layout: selects "Room Layout" preset
          → Wide canvas view of the house floorplan
          → Drags each WLED device to its physical location
          → Kitchen devices clustered together, bedroom separate
          → Garage strip runs along one wall

T+5:00    Tests an audio-reactive effect across all 30 devices
          → DDP to all 30 simultaneously
          → Latency: 3-8ms per device (WiFi)
          → 2 devices showing 15ms+ latency (WiFi congestion)
            → Stability monitor suggests: "Consider wired Ethernet
              for these ESP32s"

T+6:00    Creates profiles:
          → "Movie Mode" — dim warm glow across living room
          → "Party Mode" — full audio-reactive rainbow everywhere
          → "Sleep Mode" — bedroom devices only, slow warm breathing

T+8:00    Done. 30 WLED devices across 2 VLANs, all named,
          all positioned on the room layout, 3 profiles ready.
```

---

## Appendix A: Data Flow Summary

```
┌─────────────┐
│   Install    │
│  (package    │
│   manager)   │
└──────┬──────┘
       │ udev rules auto-installed
       ▼
┌──────────────┐     ┌──────────────┐     ┌──────────────┐
│  USB Enum    │     │  mDNS Browse │     │  OpenRGB SDK │
│  (hidapi +   │     │  (mdns-sd)   │     │  (TCP 6742)  │
│   udev)      │     │              │     │              │
└──────┬───────┘     └──────┬───────┘     └──────┬───────┘
       │                    │                    │
       └────────┬───────────┴────────────────────┘
                │
       ┌────────▼─────────┐
       │  Discovery        │
       │  Orchestrator     │
       │  (dedup + diff)   │
       └────────┬──────────┘
                │
       ┌────────▼──────────┐
       │  Device Registry  │  ~/.config/hypercolor/devices.toml
       │  (persistent)     │
       └────────┬──────────┘
                │
       ┌────────▼──────────┐
       │  Permission Check │
       │  (udev, i2c,     │
       │   audio)          │
       └────────┬──────────┘
                │
       ┌────────▼──────────┐
       │  Auto-Configure   │  Match against device database
       │  (LED counts,     │  resources/devices/*.toml
       │   color format,   │
       │   topology)       │
       └────────┬──────────┘
                │
       ┌────────▼──────────┐
       │  Device Test      │  Identify flash + color sweep
       │  (confirm working)│
       └────────┬──────────┘
                │
       ┌────────▼──────────┐
       │  Spatial Layout   │  Place on canvas
       │  (position LEDs)  │
       └────────┬──────────┘
                │
       ┌────────▼──────────┐
       │  Render Loop      │  Effect → canvas → sample → push
       │  (60fps)          │
       └──────────────────┘
```

## Appendix B: Error Taxonomy

| Error Code | Category | Description | User-Facing Message | Auto-Recovery |
|---|---|---|---|---|
| `E001` | Permission | HID device permission denied | "USB device needs udev rules" | No (needs sudo) |
| `E002` | Permission | i2c-dev module not loaded | "Kernel module needed for motherboard RGB" | No (needs sudo) |
| `E003` | Permission | Audio capture failed | "Can't access audio — check PipeWire config" | Retry on reconnect |
| `E010` | Hardware | USB device disconnected | "Device unplugged" | Yes (hot-plug monitor) |
| `E011` | Hardware | USB communication timeout | "Device not responding" | Yes (retry with backoff) |
| `E012` | Hardware | Wrong LED count | "Expected 126 LEDs, got 42" | No (manual override) |
| `E020` | Network | WLED device unreachable | "WiFi device offline" | Yes (health monitor) |
| `E021` | Network | mDNS resolution failed | "Can't resolve device hostname" | Yes (retry) |
| `E022` | Network | VLAN boundary | "Device on different network" | No (manual IP entry) |
| `E030` | Protocol | Unexpected firmware response | "Device response doesn't match protocol" | No (report bug) |
| `E031` | Protocol | Color format mismatch | "Colors appear wrong (RGB/GRB swap)" | Yes (auto-detect) |
| `E040` | OpenRGB | Daemon not running | "OpenRGB not detected" | Yes (retry on rescan) |
| `E041` | OpenRGB | Device conflict | "Both Hypercolor and OpenRGB controlling same device" | No (user choice) |
| `E042` | OpenRGB | Protocol version mismatch | "OpenRGB version too old" | No (upgrade OpenRGB) |

## Appendix C: Configuration Reference

```toml
# ~/.config/hypercolor/config.toml

[discovery]
# Auto-scan on startup
auto_scan = true
# Scan interval for network devices (seconds)
network_scan_interval = 30
# mDNS service types to browse
mdns_services = ["_wled._tcp", "_hue._tcp"]
# Additional subnets to scan (for VLANs)
extra_subnets = ["192.168.10.0/24"]

[openrgb]
# OpenRGB daemon endpoint
endpoint = "127.0.0.1:6742"
# Auto-connect if available
auto_connect = true
# Prefer OpenRGB for devices it supports
prefer_openrgb = true

[permissions]
# Path to udev rules file
udev_rules_path = "/etc/udev/rules.d/99-hypercolor.rules"
# Automatically check permissions on startup
auto_check = true

[hotplug]
# Enable USB hot-plug monitoring
usb_monitor = true
# Network health check interval (seconds)
health_check_interval = 10
# Reconnect policy
reconnect_initial_delay_ms = 1000
reconnect_max_delay_ms = 60000
reconnect_backoff_factor = 2.0

[testing]
# Identify flash color (SilkCircuit neon cyan)
identify_color = [128, 255, 234]
# Color sweep duration per color (ms)
sweep_duration_ms = 500
```
