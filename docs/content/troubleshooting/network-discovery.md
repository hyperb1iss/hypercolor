+++
title = "Network device not discovered"
description = "VLAN/AP isolation, mDNS not crossing subnets, firewall blocking multicast, and the known-IP escape hatch."
weight = 30
+++

Network devices (Hue, Nanoleaf, WLED, Govee) rely on mDNS multicast for automatic discovery. Three network configurations reliably break that: **AP/client isolation**, **VLAN segmentation**, and **multicast firewall rules**. This page walks through diagnosing which one you have and how to work around it using per-driver static IP lists.

![Device discovery in the Hypercolor web UI](/img/ui/ui-devices.webp)

## How discovery works

When a scan runs — either at startup, on a periodic interval, or triggered manually — each network driver uses mDNS multicast on `224.0.0.251:5353`. Devices on the same Layer 2 segment respond; the daemon registers them and keeps them connected. If mDNS reaches the device, discovery is automatic and requires no configuration.

The daemon also advertises itself using the service type `_hypercolor._tcp.local.` via the `network.mdns_publish` config flag (default `true`). If you can resolve that name from another machine on the same subnet, basic mDNS is working.

Discovery is controlled by `discovery.mdns_enabled` in `hypercolor.toml` (default `true`). Disabling it suppresses the mDNS listener but does not affect direct IP probe paths — those run regardless.

Default scan timeout is **10 seconds**, clamped between 100 ms and 60 seconds.

## Diagnosing the problem

Before touching config, confirm the device is actually reachable at the IP level:

```bash
# Replace 192.168.1.42 with your device's IP
ping -c 3 192.168.1.42
```

If `ping` fails, the issue is IP routing, not mDNS. Fix the network path first.

If `ping` succeeds but discovery still misses the device, run a targeted scan and watch the result:

```bash
hypercolor devices discover --target wled --timeout 15
```

The scan returns a `scan_id` and `status: scanning`. After it completes, list devices:

```bash
hypercolor devices list
```

For the full machine-readable scan result including per-scanner diagnostics, use the API directly with `wait: true`:

```bash
curl -s -X POST http://localhost:9420/api/v1/devices/discover \
  -H 'Content-Type: application/json' \
  -d '{"targets": ["wled"], "timeout_ms": 15000, "wait": true}'
```

The response includes a `scanners` array with each scanner's duration, discovered count, and any error message.

## Common causes

### AP/client isolation

Consumer Wi-Fi access points and most enterprise Wi-Fi controllers support **client isolation** (also called AP isolation or wireless isolation). When enabled, this drops all Layer 2 traffic between wireless clients, including mDNS multicast. Your host machine and your WLED controller may be on the same SSID but cannot reach each other.

**Fix:** Disable client isolation in your AP's settings. The option is usually under Wireless > Advanced or a "Client Isolation" toggle. On enterprise gear (UniFi, Meraki, Aruba) it is a per-SSID or per-WLAN policy.

If you cannot change the AP policy, use the [known-IP escape hatch](#the-known-ip-escape-hatch) below.

### mDNS not crossing subnets

mDNS is link-local by definition: multicast packets with TTL=1 die at the first router hop. If your devices are on a different subnet than the machine running Hypercolor — even a VLAN on the same physical switch — mDNS will not cross that boundary without a multicast proxy.

**Fix:** Run an mDNS reflector or proxy on your router to bridge the two segments. Common options:

- **Avahi** (`avahi-daemon --reflector`) on Linux routers
- **mdns-repeater** on OpenWrt
- **Bonjour Proxy** on UniFi (Settings > Networks > your VLAN > mDNS)

Alternatively, move the device to the same subnet as the Hypercolor host, or use the [known-IP escape hatch](#the-known-ip-escape-hatch).

### Multicast blocked by firewall rules

Some firewall configurations block `224.0.0.251` (the mDNS group address) or all multicast traffic on the interface. This is common on hardened server setups or when iptables/nftables rules are generated automatically.

**Diagnosis:**

```bash
# Check if mDNS traffic is reaching the interface
sudo tcpdump -i <interface> port 5353 -n
```

Trigger a scan from another terminal while `tcpdump` is running. If you see outgoing queries but no replies, the device is not responding (or AP isolation is blocking it). If you see neither, the kernel or a firewall rule is dropping the multicast traffic outbound.

**Fix on Linux (iptables):**

```bash
sudo iptables -A OUTPUT -p udp --dport 5353 -d 224.0.0.251 -j ACCEPT
sudo iptables -A INPUT  -p udp --sport 5353 -s 224.0.0.0/4  -j ACCEPT
```

Persist via your distribution's iptables service or an equivalent nftables ruleset.

## The known-IP escape hatch

When mDNS is unavailable and you cannot fix the network topology, each network driver supports a list of IPs that are always probed directly during discovery — no multicast required. Add these in `~/.config/hypercolor/hypercolor.toml` (Linux) or `%APPDATA%\hypercolor\hypercolor.toml` (Windows).

The key name differs per driver:

### WLED

```toml
[drivers.wled]
known_ips = ["192.168.10.50", "192.168.10.51"]
```

WLED merges `known_ips` with any cached probe results from previous scans, so the list only needs to contain devices not already known.

### Govee

```toml
[drivers.govee]
known_ips = ["192.168.10.60"]
```

The Govee driver uses LAN UDP for local control. Devices in `known_ips` are always probed; cloud-only SKUs still require the Govee Cloud API regardless of IP reachability.

### Nanoleaf

Nanoleaf uses `device_ips` rather than `known_ips`:

```toml
[drivers.nanoleaf]
device_ips = ["192.168.10.70"]
```

Devices at these addresses are contacted directly; mDNS is used in addition when available.

### Philips Hue

Hue uses `bridge_ips` to locate the Hue Bridge when mDNS is unavailable:

```toml
[drivers.hue]
bridge_ips = ["192.168.10.1"]
```

Only the Bridge IP is needed; individual Hue lights are enumerated from the bridge automatically after pairing.

After editing the config, trigger a fresh scan to pick up the statically configured devices:

```bash
hypercolor devices discover --timeout 15
```

## Checking driver enabled state

If you scan a specific target and get an error like `Discovery target 'wled' is disabled by config (drivers.wled.enabled=false)`, the driver has been explicitly disabled. Re-enable it:

```toml
[drivers.wled]
enabled = true
```

Drivers default to enabled unless explicitly set otherwise. The error message always names the exact config key to change.

## Disabling mDNS entirely

If your network has no mDNS infrastructure and you are using static IPs for all devices, you can turn off the mDNS listener to reduce startup noise:

```toml
[discovery]
mdns_enabled = false
```

This does not affect the known-IP probe path — direct IP probes run regardless of this flag.

## Still not found?

{% callout(type="tip") %}
After adding static IPs, run `hypercolor devices discover --timeout 15` or higher. Some devices (especially Govee on cold probes) take longer than the 10-second default.
{% end %}

If a device appears in the scan result but immediately disappears, it likely requires pairing before it will accept connections. Run:

```bash
hypercolor devices pair <device-name-or-id>
```

For device-specific pairing flows and known quirks, see the hardware pages:

- [Philips Hue](@/hardware/hue.md)
- [Nanoleaf](@/hardware/nanoleaf.md)
- [WLED](@/hardware/wled.md)
- [Govee](@/hardware/govee.md)

For general device-not-found problems covering USB and SMBus devices as well, see [Devices not found](@/troubleshooting/devices-not-found.md).
