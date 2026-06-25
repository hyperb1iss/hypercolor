+++
title = "Adding a network driver"
description = "How to implement a Hypercolor network driver: the driver-api boundary, DDP/E1.31/HTTP pairing patterns, using WLED and Govee as reference."
weight = 40
+++

# Adding a network driver

Network drivers live in their own crates behind the stable `hypercolor-driver-api` boundary.
They depend on `hypercolor-driver-api` and `hypercolor-types` — never on `hypercolor-core`
directly. The `hypercolor-network` crate holds only the registry and capability-dispatch
shell; protocol logic stays in each driver crate. This page walks through the full lifecycle
of adding a new one, using WLED and Govee as concrete reference implementations.

---

## The driver-api boundary

`hypercolor-driver-api` defines every trait and type the daemon needs from a driver. The
dependency rule is strict:

```
hypercolor-types ──▶ hypercolor-driver-api ──▶ your-driver-crate
                                              ╰──▶ hypercolor-network (registry)
                                              ╰──▶ hypercolor-driver-builtin (bundle)
```

Never reach into `hypercolor-core` from a driver crate. Core depends on `driver-api`, not
the reverse. The `hypercolor-network` crate is only the `DriverModuleRegistry` orchestration
layer — it does not own any protocol logic.

### Key types at a glance

| Type | Module | Role |
|---|---|---|
| `DriverModule` | `driver_api::module` | Capability root — the entry point the host sees |
| `DriverDescriptor` | `driver_api::descriptor` | Static ID, display name, transport kind, schema version |
| `DeviceBackend` | `driver_api::backend` | Hot-path trait: `discover`, `connect`, `disconnect`, `write_colors` |
| `DiscoveryCapability` | `driver_api::driver_discovery` | Async scan returning `DiscoveryResult` |
| `PairingCapability` | `driver_api::pairing` | `auth_summary`, `pair`, `clear_credentials` |
| `DriverControlProvider` | `driver_api::controls` | Driver-level and device-level control surfaces |
| `DriverRuntimeCacheProvider` | `driver_api::module` | Persist discovery hints across daemon restarts |
| `DriverModuleRegistry` | `hypercolor_network` | Host-side lookup and capability dispatch |

---

## Declaring a descriptor

Every driver crate exposes a `static DESCRIPTOR` at crate root. `DriverDescriptor::new`
stamps it with the current `DRIVER_API_SCHEMA_VERSION` automatically; the registry rejects
mismatches at load time.

```rust
use hypercolor_driver_api::DriverDescriptor;
use hypercolor_types::device::DriverTransportKind;

pub static DESCRIPTOR: DriverDescriptor =
    DriverDescriptor::new("acme", "Acme Lights", DriverTransportKind::Network, true, false);
//                         id      display_name    transport             discovery pairing
```

Set `supports_discovery` to `true` if you implement `DiscoveryCapability`. Set
`supports_pairing` to `true` if you implement `PairingCapability`. Govee does; WLED does not
(no auth required).

---

## Implementing DriverModule

`DriverModule` is the capability root. Implement it on a struct that carries any
compile-time configuration the module needs — for example, whether mDNS browsing is enabled.

```rust
use hypercolor_driver_api::{
    DeviceBackend, DiscoveryCapability, DriverConfigProvider, DriverConfigView,
    DriverDescriptor, DriverHost, DriverModule,
};

pub struct AcmeDriverModule {
    mdns_enabled: bool,
}

impl DriverModule for AcmeDriverModule {
    fn descriptor(&self) -> &'static DriverDescriptor {
        &DESCRIPTOR
    }

    fn has_output_backend(&self) -> bool {
        true
    }

    fn build_output_backend(
        &self,
        host: &dyn DriverHost,
        config: DriverConfigView<'_>,
    ) -> anyhow::Result<Option<Box<dyn DeviceBackend>>> {
        let cfg = config.parse_settings::<AcmeConfig>()?;
        Ok(Some(Box::new(AcmeBackend::new(cfg, self.mdns_enabled))))
    }

    fn discovery(&self) -> Option<&dyn DiscoveryCapability> {
        Some(self)
    }

    fn config(&self) -> Option<&dyn DriverConfigProvider> {
        Some(self)
    }
}
```

Methods you do not need (`pairing`, `controls`, `runtime_cache`, `presentation`,
`protocol_catalog`) have default implementations that return `None`. Override only what you
support; `DriverModule::module_descriptor` assembles the capability set automatically from
those `Option` returns.

---

## Implementing DeviceBackend

`DeviceBackend` is the hot-path trait the render loop calls on every frame. It is
`Send + Sync` and must not block the async executor.

```rust
use hypercolor_driver_api::{BackendInfo, DeviceBackend, OutputCadence};
use hypercolor_types::device::{DeviceId, DeviceInfo};

#[async_trait::async_trait]
impl DeviceBackend for AcmeBackend {
    fn info(&self) -> BackendInfo {
        BackendInfo {
            id: "acme".into(),
            name: "Acme Lights".into(),
            description: "Acme UDP streaming backend".into(),
        }
    }

    async fn discover(&mut self) -> anyhow::Result<Vec<DeviceInfo>> {
        // Scan the network and return DeviceInfo for each reachable device.
        // Fingerprint on MAC address so DHCP changes don't lose the device.
        todo!()
    }

    async fn connect(&mut self, id: &DeviceId) -> anyhow::Result<()> {
        // Open the UDP socket or handshake as required.
        todo!()
    }

    async fn disconnect(&mut self, id: &DeviceId) -> anyhow::Result<()> {
        todo!()
    }

    async fn write_colors(&mut self, id: &DeviceId, colors: &[[u8; 3]]) -> anyhow::Result<()> {
        // Encode and send one frame. Called on every render tick; keep allocations minimal.
        todo!()
    }

    fn output_cadence(&self, id: &DeviceId) -> Option<OutputCadence> {
        // Govern how often the render loop sends frames to this device.
        Some(OutputCadence::from_fps(30))
    }
}
```

`DeviceLifecyclePolicy::default()` gives a 5-second connect timeout with inline execution
and retry on timeout. Override `lifecycle_policy` only when your connect call blocks for
several seconds and you need `ConnectExecution::Background` — for example, a DTLS handshake
like Hue's Entertainment API.

---

## Discovery patterns

### WLED: mDNS + HTTP enrichment

WLED discovers via mDNS service type `_wled._tcp.local.` and then enriches each result with
a `GET http://<ip>/json/info` call that returns firmware version, LED count, RGBW flag, and
max FPS. It also probes a list of known IPs for controllers that do not advertise via mDNS,
which is common on networks with multicast filtering.

The `DiscoveryCapability` implementation loads cached probe targets from the previous run via
`DriverHost::discovery_state().load_cached_json(...)`, merges them with the current config
and tracked devices, then passes the combined list to the scanner:

```rust
#[async_trait::async_trait]
impl DiscoveryCapability for WledDriverModule {
    async fn discover(
        &self,
        host: &dyn DriverHost,
        request: &DiscoveryRequest,
        config: DriverConfigView<'_>,
    ) -> anyhow::Result<DiscoveryResult> {
        let config = config.parse_settings::<WledConfig>()?;
        let tracked = host.discovery_state().tracked_devices(DESCRIPTOR.id).await;
        let known_targets = resolve_wled_probe_targets_from_sources(
            &config, &tracked, &cached_probe_ips, &cached_targets,
        );
        let mut scanner = WledScanner::with_known_targets(
            known_targets, request.mdns_enabled, request.timeout,
        );
        let devices = scanner
            .scan()
            .await?
            .into_iter()
            .map(DriverDiscoveredDevice::from)
            .collect();
        Ok(DiscoveryResult { devices })
    }
}
```

Device fingerprints use the MAC address (`net:<mac>`) rather than the IP address, so a DHCP
reassignment keeps the same `DeviceId`.

### Govee: UDP multicast + optional cloud fallback

Govee has no mDNS. Discovery sends a JSON scan command to the multicast address
`239.255.255.250:4001`; devices respond on port `4002`. Each device is then reachable for
control on port `4003`.

After the LAN scan, if the user has stored an API key, the driver calls the Govee Developer
Cloud API (`list_v1_devices`) and merges any cloud-only devices into the result set. Cloud
devices whose MAC matches a LAN entry are merged in-place; cloud-only devices are added with
`DiscoveryConnectBehavior::Deferred` because they cannot be LAN-controlled without an IP.

```rust
// After the LAN scan completes:
if let Some(api_key) = account_api_key(host).await? {
    match self.cloud_client(api_key)?.list_v1_devices().await {
        Ok(cloud_devices) => merge_cloud_inventory(&mut devices, cloud_devices),
        Err(error) => warn!(error = %error, "Govee cloud inventory failed"),
    }
}
```

The cloud call is best-effort: a failure logs a warning and does not fail the scan.

{% callout(type="tip") %}
mDNS frequently fails on managed networks with multicast filtering, across VLANs, or when
`systemd-resolved` is running as a stub. Always provide a `known_ips` config field as an
escape hatch so users can add IPs manually. WLED, Hue, and Nanoleaf all follow this pattern.
{% end %}

---

## Pairing

Drivers that require credentials implement `PairingCapability`. Govee is the reference; WLED
intentionally sets `supports_pairing: false` because it requires no authentication.

The three methods:

```rust
#[async_trait::async_trait]
impl PairingCapability for GoveeDriverModule {
    async fn auth_summary(
        &self,
        host: &dyn DriverHost,
        device: &TrackedDeviceCtx<'_>,
    ) -> Option<DeviceAuthSummary> {
        // Inspect host.credentials() and return the current auth state.
        // Return None only if this driver cannot describe auth for this device at all.
    }

    async fn pair(
        &self,
        host: &dyn DriverHost,
        device: &TrackedDeviceCtx<'_>,
        request: &PairDeviceRequest,
    ) -> anyhow::Result<PairDeviceOutcome> {
        // Validate the credential against the remote service, then store it.
        let api_key = api_key_from_request(request)
            .ok_or_else(|| anyhow::anyhow!("API key is required."))?;
        self.cloud_client(api_key.clone())?
            .list_v1_devices()
            .await?;                          // validation call — fails fast on bad key
        host.credentials()
            .set_json(DESCRIPTOR.id, "account", serde_json::json!({ "api_key": api_key }))
            .await?;
        Ok(PairDeviceOutcome {
            status: PairDeviceStatus::Paired,
            message: "API key validated and stored.".into(),
            auth_state: DeviceAuthState::Configured,
            activated: false,
        })
    }

    async fn clear_credentials(
        &self,
        host: &dyn DriverHost,
        device: &TrackedDeviceCtx<'_>,
    ) -> anyhow::Result<ClearPairingOutcome> {
        host.credentials().remove(DESCRIPTOR.id, "account").await?;
        Ok(ClearPairingOutcome {
            message: "Credentials removed.".into(),
            auth_state: DeviceAuthState::Open,
            disconnected: false,
        })
    }
}
```

Describe the pairing flow to the UI by returning a `PairingDescriptor` from `auth_summary`.
Govee uses `PairingFlowKind::CredentialsForm` with step-by-step instructions:

```rust
PairingDescriptor {
    kind: PairingFlowKind::CredentialsForm,
    title: "Pair Govee Account".into(),
    instructions: vec![
        "Open the Govee Home app.".into(),
        "Go to Profile, Settings, Apply for API Key.".into(),
        "Paste the API key here to validate it and unlock cloud fallback.".into(),
    ],
    action_label: "Validate API Key".into(),
    fields: vec![PairingFieldDescriptor {
        key: "api_key".into(),
        label: "Govee API Key".into(),
        secret: true,
        optional: false,
        placeholder: Some("xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx".into()),
    }],
}
```

---

## Streaming protocols

### DDP (WLED default)

DDP (Distributed Display Protocol) sends pixel data as a UDP datagram to port `4048`. The
payload is capped at `1440` bytes to stay under the 1472-byte UDP MTU, limiting a single
packet to 480 RGB pixels or 360 RGBW pixels. For longer strips the implementation segments
across multiple packets using sequential push flags.

DDP data types: `0x0B` for RGB24, `0x1B` for RGBW32. The WLED driver exposes a per-device
protocol override so individual controllers can run E1.31 while the rest use DDP.

### E1.31 / sACN

E1.31 is the sACN (Streaming ACN) protocol used in professional lighting. It spreads pixels
across DMX universes: one universe carries 512 channels, mapping to 170 RGB pixels or 127
RGBW pixels. Use E1.31 only when integrating with an existing sACN workflow. For most
setups, DDP is preferred — it is simpler and carries a full strip in fewer packets.

### Govee LAN UDP

Govee uses a JSON-over-UDP control protocol on port `4003`. For SKUs with the
`RAZER_STREAMING` capability flag, the driver encodes pixel data as base64 frames at up to
25 fps, with a cap of 255 LEDs per frame. Plain LAN devices without Razer streaming support
run at up to 10 fps.

### Shared HTTP clients

Drivers that make HTTP calls for pairing or discovery enrichment (WLED's `GET /json/info`,
Govee's cloud API) should share a single `reqwest::Client` rather than building one per
call. Store it in a `LazyLock` at crate root:

```rust
use std::sync::LazyLock;
use std::time::Duration;

static HTTP_CLIENT: LazyLock<Result<reqwest::Client, String>> = LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|e| e.to_string())
});

fn http_client() -> anyhow::Result<&'static reqwest::Client> {
    HTTP_CLIENT
        .as_ref()
        .map_err(|e| anyhow::anyhow!("HTTP client unavailable: {e}"))
}
```

---

## Config and control surfaces

Expose driver-level configuration via `DriverConfigProvider`. The `default_config` method
returns a `DriverConfigEntry::enabled(...)` with a `BTreeMap<String, serde_json::Value>` of
default settings. `validate_config` parses and validates the config on user changes.

WLED's driver-level config covers `known_ips`, `default_protocol`, `realtime_http_enabled`,
and `dedup_threshold`. Per-device overrides (for example, an individual WLED controller using
E1.31 while others use DDP) are exposed separately through
`DriverControlProvider::device_surface`.

`ApplyImpact` tells the host what needs to happen when a field changes:

| Variant | Meaning |
|---|---|
| `ApplyImpact::Live` | Takes effect immediately, no reconnect |
| `ApplyImpact::BackendRebind` | Backend must be torn down and rebuilt |
| `ApplyImpact::DeviceReconnect` | Only the affected device reconnects |
| `ApplyImpact::DiscoveryRescan` | A fresh discovery scan should run |

---

## Runtime cache

Implement `DriverRuntimeCacheProvider` so discovered device addresses survive daemon
restarts. The `snapshot` method serializes whatever you need into a
`BTreeMap<String, serde_json::Value>`; the host writes it to disk and feeds it back via
`DriverHost::discovery_state().load_cached_json(...)` on the next boot.

WLED serializes `probe_ips` and `probe_targets`. Govee serializes `probe_devices`. Keep the
cached data compact — it is loaded synchronously at discovery startup.

```rust
#[async_trait::async_trait]
impl DriverRuntimeCacheProvider for AcmeDriverModule {
    async fn snapshot(
        &self,
        host: &dyn DriverHost,
    ) -> anyhow::Result<BTreeMap<String, serde_json::Value>> {
        let tracked = host.discovery_state().tracked_devices(DESCRIPTOR.id).await;
        let probe_ips = collect_probe_ips(&tracked);
        Ok(BTreeMap::from([(
            "probe_ips".into(),
            serde_json::to_value(probe_ips)?,
        )]))
    }
}
```

---

## Registering in the bundle

Add your crate as an optional feature-gated dependency of `hypercolor-driver-builtin` and
register the module in `register_driver_modules`:

```rust
// In hypercolor-driver-builtin/src/lib.rs

#[cfg(feature = "acme")]
use hypercolor_driver_acme::AcmeDriverModule;

// Inside register_driver_modules():
#[cfg(feature = "acme")]
registry.register(AcmeDriverModule::new(config.discovery.mdns_enabled))?;
```

Also add a `normalize_driver_config_entries` call so the driver's config entry is created on
first run:

```rust
#[cfg(feature = "acme")]
config
    .drivers
    .entry(hypercolor_driver_acme::DESCRIPTOR.id.to_owned())
    .or_default();
```

`DriverModuleRegistry::register` verifies that your `DriverDescriptor::schema_version`
matches `DRIVER_API_SCHEMA_VERSION`. A mismatch returns
`DriverModuleRegistryError::SchemaVersionMismatch` and prevents the daemon from starting.

---

## Tests

Tests live in `crates/hypercolor-driver-<name>/tests/` — not inline `#[cfg(test)]` blocks.
Name each file after the capability being tested: `discovery_tests.rs`, `pairing_tests.rs`,
and so on.

At minimum, cover:

- **Config round-trips**: `default_config` deserializes cleanly via `parse_settings`.
- **Known-IP merging**: `resolve_*_probe_targets_from_sources` merges config, tracked
  devices, and cache without duplicates.
- **Fingerprint stability**: the same MAC address produces the same `DeviceId` across
  independent calls.
- **Pairing validation**: missing fields and empty credentials return
  `PairDeviceStatus::InvalidInput`, not an error.

```bash
just test-crate hypercolor-driver-acme
```

{% callout(type="info") %}
`cargo check --workspace` does not cover `hypercolor-ui`, but it does cover your new driver
crate as long as you add it to the workspace `Cargo.toml`. Run `just verify` (fmt + lint +
test) before opening a PR.
{% end %}

---

## Cross-links

- Network device ports and mDNS service types: [@/hardware/network-devices.md](@/hardware/network-devices.md)
- WLED setup and streaming parameters: [@/hardware/wled.md](@/hardware/wled.md)
- Govee setup, LAN ports, and cloud pairing: [@/hardware/govee.md](@/hardware/govee.md)
- HAL (USB/HID/SMBus) driver contribution guide: [@/contributing/adding-a-driver.md](@/contributing/adding-a-driver.md)
- Render pipeline and `BackendManager::write_frame`: [@/architecture/render-pipeline.md](@/architecture/render-pipeline.md)
