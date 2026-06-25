+++
title = "Network devices"
description = "How Hypercolor discovers and pairs Wi-Fi and LAN RGB devices: mDNS, known-IP, and manual discovery, plus per-vendor links."
weight = 40
+++

Network RGB devices (Philips Hue, Nanoleaf, WLED, and Govee) connect over your local network rather than USB. Hypercolor handles discovery, pairing, and high-frequency streaming entirely on the LAN; no cloud services are required for any of the four built-in network drivers.

![Device discovery in the Hypercolor web UI](/img/ui/ui-devices.webp)

## How discovery works

Hypercolor uses three complementary methods to find network devices. All three run in parallel during a single scan.

**mDNS (zero-configuration)** is the primary path. Each driver listens for its vendor-specific service type on the local network. When a compatible device announces itself, Hypercolor receives the advertisement, optionally enriches it with a device-info HTTP probe, and adds the device to the registry.

| Driver | mDNS service type | Notes |
|--------|------------------|-------|
| Hue | `_hue._tcp.local.` | Falls back to N-UPnP (`discovery.meethue.com`) if mDNS returns nothing |
| Nanoleaf | `_nanoleafapi._tcp.local.` | TXT record carries model name, firmware, and panel count |
| WLED | `_wled._tcp.local.` | Enriched via HTTP `/json/info` after advertisement |
| Govee | none (UDP multicast) | Govee uses a proprietary UDP scan on `239.255.255.250:4001`; mDNS is not involved |

mDNS only works when devices and the Hypercolor host share a broadcast domain. VLANs, AP isolation, and some router firmware block it — see [mDNS troubleshooting](#mdns-troubleshooting) below.

**Known-IP probing** is the escape hatch when mDNS is blocked. You can supply a static list of IP addresses for each driver in the Hypercolor config file. The daemon probes those addresses directly on startup and during every scan, bypassing mDNS entirely. See each vendor's setup page for the exact config keys.

**Manual target scoping** lets you tell a scan which drivers to run. By default a scan covers all enabled drivers. Naming specific targets narrows the scope:

```bash
# scan only WLED and Hue, 20-second window
hypercolor devices discover --target wled --target hue --timeout 20
```

The `--target` flag accepts any driver module ID (`wled`, `hue`, `nanoleaf`, `govee`) and the built-in local transport IDs (`usb`, `smbus`). You can mix them freely. Omitting `--target` runs everything.

The `--timeout` value is in seconds and defaults to 10. The daemon clamps it between 0.1 s and 60 s regardless of what you pass.

### REST API

`POST /api/v1/devices/discover` triggers the same scan programmatically.

```json
{
  "targets": ["wled", "nanoleaf"],
  "timeout_ms": 15000,
  "wait": true
}
```

All three fields are optional. When `wait` is `false` (the default), the endpoint returns immediately with a `scan_id` and `"status": "scanning"`. Set `wait: true` to block until the scan completes and receive the full result in the response body.

{% callout(type="warning") %}
Only one discovery scan can run at a time. A second `POST /api/v1/devices/discover` while a scan is active returns **409 Conflict**. Wait for the current scan to finish, or increase `timeout_ms` on the first call to cover the full window you need.
{% end %}

### Device fingerprinting and IP changes

Network devices are fingerprinted on stable hardware identity rather than their current IP address:

- WLED — MAC address (`net:<mac>`)
- Govee — MAC address (`net:govee:<mac>`)
- Hue — Bridge ID
- Nanoleaf — device serial key

A DHCP address change does not lose pairing or rename the device in Hypercolor's registry. Only a factory reset or key regeneration on the device side requires re-pairing.

---

## Pairing and credentials

Some network drivers require an explicit pairing step before Hypercolor can send lighting data. The daemon stores credentials encrypted and loads them automatically on startup — you pair once per device.

| Driver | Pairing required | What pairing obtains |
|--------|-----------------|----------------------|
| WLED | No | WLED has no authentication. Discovery is the only step. |
| Hue | Yes — link button on Bridge | `username` (used as the DTLS PSK identity) and `clientkey` (the PSK hex string). Both are required to open the Entertainment API streaming connection on UDP port 2100. |
| Nanoleaf | Yes — hold power button 5–7 s | Posts to `/api/v1/new` and receives `auth_token`, stored and used for REST calls and to enable UDP External Control on port 60222. |
| Govee | Optional — Developer API key | LAN UDP control works without a key. The API key enables cloud fallback for devices that don't support the LAN protocol. |

To pair a device that was discovered but is not yet streaming:

```bash
hypercolor devices pair "<device name or ID>"
```

`hypercolor devices list` shows each device's state. A device stuck in `discovered` is waiting for credentials. A `connected` device is actively streaming.

{% callout(type="info") %}
Credentials survive daemon restarts. Re-pairing is only needed if you factory-reset the device or regenerate its API keys.
{% end %}

{% callout(type="tip") %}
If you're not sure which device is which, `hypercolor devices identify "<name>"` flashes a test pattern on that device for 5 seconds. Adjust the duration with `--duration`.
{% end %}

---

## mDNS troubleshooting

When `hypercolor devices discover` returns nothing, the most common causes on a LAN are:

**AP isolation / client isolation** blocks multicast and mDNS between wireless clients and the Hypercolor host. Disable it for the VLAN that contains your RGB devices, or check your access point's "wireless isolation" setting.

**Separate VLANs or subnets** prevent mDNS from crossing the network boundary without a proxy. Either place the Hypercolor host and RGB devices on the same subnet, or configure mDNS reflection via `avahi-daemon --reflector` or a router-level mDNS proxy.

**`systemd-resolved` stub resolver** can conflict with `avahi-daemon`. Check whether mDNS is enabled on the relevant interface:

```bash
resolvectl status
```

Look for `MulticastDNS: yes` on the interface your RGB devices are on. If it shows `no`, you may need to set `MulticastDNS=yes` in `/etc/systemd/resolved.conf` or in the per-interface `.network` configuration.

The known-IP fallback bypasses all of these. Consult each vendor's dedicated page for the exact config keys to set static addresses.

If you want to disable mDNS globally in Hypercolor while keeping known-IP probing active, set `discovery.mdns_enabled = false` in your Hypercolor config.

See also: [network discovery troubleshooting](@/troubleshooting/network-discovery.md) for deeper diagnostic steps and log targets.

---

## Per-vendor setup

Each network driver has its own pairing flow, streaming protocol, supported device SKUs, and common failure modes. Start with the vendor page for your hardware.

- [Philips Hue](@/hardware/hue.md) — mDNS and N-UPnP discovery, link-button pairing (30-second window), Entertainment API DTLS streaming on UDP port 2100.
- [Nanoleaf](@/hardware/nanoleaf.md) — mDNS discovery, power-button token pairing (hold 5–7 s), UDP External Control on port 60222.
- [WLED](@/hardware/wled.md) — mDNS discovery, no authentication required, DDP streaming on port 4048 by default, or E1.31/sACN for multi-controller setups.
- [Govee](@/hardware/govee.md) — proprietary UDP multicast discovery, LAN control must be enabled per-device in the Govee Home app, optional Developer API key for cloud fallback.

For devices that don't have a native Hypercolor driver, the [OpenRGB fallback](@/hardware/openrgb-fallback.md) bridge can control them through a user-managed OpenRGB server on TCP port 6742.

---

## Devices on the same host machine

If the Hypercolor daemon and the RGB device software both run on the same machine, they may fight over the same network socket or claim the same control channel. This is uncommon for LAN devices (each speaks to a dedicated IP) but worth knowing: only one application should be issuing streaming data to any given device at a time. Mixed control produces undefined color output.

For USB devices on the same machine, see [conflicting software](@/hardware/conflicting-software.md).
