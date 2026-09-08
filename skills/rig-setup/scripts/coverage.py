#!/usr/bin/env python3
"""Who drives what: native, OpenRGB bridge, unclaimed, or conflict.

    coverage.py [--base URL] [--json] [--all] [--no-host-scan]

Asks the daemon for GET /devices/coverage and prints one line per physical device,
grouped by who owns it. On a daemon that predates that route (it answers 404 as
`device_not_found`, because the path falls into /devices/{id}), the same picture is
rebuilt from what every daemon has: GET /devices for native and bridged devices,
GET /devices/unclaimed when it exists, and otherwise the host's own USB inventory
(lsusb or sysfs, `system_profiler SPUSBDataType -json`, `pnputil /enum-devices
/connected`) diffed against the protocol table in GET /drivers.

Exit status is 0 whenever a report was printed, 1 when the daemon is unreachable.
Pure standard library, Python 3.11+.
"""
from __future__ import annotations

import argparse
import json
import platform
import re
import shutil
import subprocess
import sys
import urllib.error
import urllib.request
from pathlib import Path

DEFAULT_BASE = "http://localhost:9420/api/v1"
BRIDGE_PREFIX = "openrgb:"
USB_HUB_CLASS = 9
ROOT_HUB_VENDORS = {0x1D6B}  # Linux Foundation root hubs
# Interface classes that are never RGB controllers: audio, CDC, imaging, printer, mass
# storage, CDC data, video, audio/video, application-specific (DFU, test), wireless.
# A device whose interfaces beyond HID and "per interface" are all of these is hidden
# unless --all asks for it. Pure HID (0x03) and vendor-specific (0xff) stay visible,
# since RGB controllers live there; a HID interface riding along an audio interface is a
# headset's or DAC's control surface, not lighting.
NOISE_CLASSES = {0x01, 0x02, 0x06, 0x07, 0x08, 0x0A, 0x0E, 0x10, 0xEF, 0xFE, 0xE0}


# ── HTTP ─────────────────────────────────────────────────────────────────────
class Daemon:
    def __init__(self, base: str):
        self.base = base.rstrip("/")

    def list_all(self, path: str, page_size: int = 200) -> list | None:
        """Every item of a paged list route, following `page.has_more`; None when the route is missing."""
        items: list = []
        offset = 0
        sep = "&" if "?" in path else "?"
        while True:
            doc = self.get(f"{path}{sep}limit={page_size}&offset={offset}")
            if doc is None:
                return None if not items else items
            data = doc.get("data", doc)
            if isinstance(data, list):
                return items + data
            items += data.get("items", [])
            page = data.get("page") or {}
            if not page.get("has_more") or not data.get("items"):
                return items
            offset += page.get("limit") or page_size

    def get(self, path: str):
        """GET that returns the parsed envelope, or None for a missing route (404)."""
        req = urllib.request.Request(self.base + path, method="GET")
        try:
            with urllib.request.urlopen(req, timeout=15) as resp:
                return json.load(resp)
        except urllib.error.HTTPError as err:
            if err.code == 404:
                return None
            raise SystemExit(f"GET {path} -> {err.code}: {err.read().decode()[:400]}") from err
        except urllib.error.URLError as err:
            raise SystemExit(f"daemon unreachable at {self.base} ({err.reason}); start it or pass --base") from err


def items_of(doc) -> list:
    """Unwrap ListResponse or bare-array envelopes."""
    data = doc.get("data", doc) if isinstance(doc, dict) else doc
    if isinstance(data, dict):
        return data.get("items", data.get("rows", []))
    return list(data)


# ── host USB inventory (fallback only) ───────────────────────────────────────
def parse_hex(value) -> int | None:
    if value is None:
        return None
    m = re.search(r"0x([0-9a-fA-F]{1,4})|^([0-9a-fA-F]{4})$", str(value).strip())
    if not m:
        return None
    return int(m.group(1) or m.group(2), 16)


def scan_linux() -> list[dict]:
    """sysfs first (serial and class come for free), lsusb as the fallback."""
    out = []
    root = Path("/sys/bus/usb/devices")
    if root.is_dir():
        for dev in sorted(root.iterdir()):
            vid_file = dev / "idVendor"
            if not vid_file.exists() or ":" in dev.name:
                continue

            def read(name: str) -> str:
                try:
                    return (dev / name).read_text().strip()
                except OSError:
                    return ""

            vid, pid = parse_hex(read("idVendor")), parse_hex(read("idProduct"))
            if vid is None or pid is None:
                continue
            classes = sorted({int(c, 16) for c in (read("bDeviceClass"),) if c}
                             | {int(i.read_text().strip(), 16) for i in dev.glob(f"{dev.name}:*/bInterfaceClass")
                                if i.exists()})
            out.append({"vendor_id": vid, "product_id": pid, "manufacturer": read("manufacturer"),
                        "product": read("product"), "serial": read("serial"), "bus_path": dev.name,
                        "interface_classes": classes})
        if out:
            return out
    if shutil.which("lsusb"):
        text = subprocess.run(["lsusb"], capture_output=True, text=True, check=False).stdout
        for line in text.splitlines():
            m = re.match(r"Bus (\d+) Device (\d+): ID ([0-9a-f]{4}):([0-9a-f]{4})\s*(.*)", line)
            if m:
                out.append({"vendor_id": int(m.group(3), 16), "product_id": int(m.group(4), 16),
                            "manufacturer": "", "product": m.group(5).strip(), "serial": "",
                            "bus_path": f"{m.group(1)}-{m.group(2)}", "interface_classes": []})
    return out


def scan_macos() -> list[dict]:
    if not shutil.which("system_profiler"):
        return []
    text = subprocess.run(["system_profiler", "SPUSBDataType", "-json"], capture_output=True, text=True,
                          check=False).stdout
    try:
        doc = json.loads(text or "{}")
    except json.JSONDecodeError:
        return []
    out = []

    def walk(node, path: str):
        for i, item in enumerate(node.get("_items", []) or []):
            here = f"{path}/{i}"
            vid, pid = parse_hex(item.get("vendor_id")), parse_hex(item.get("product_id"))
            if vid is not None and pid is not None:
                out.append({"vendor_id": vid, "product_id": pid, "manufacturer": item.get("manufacturer", ""),
                            "product": item.get("_name", ""), "serial": item.get("serial_num", ""),
                            "bus_path": here, "interface_classes": []})
            walk(item, here)

    for i, bus in enumerate(doc.get("SPUSBDataType", []) or []):
        walk(bus, f"bus{i}")
    return out


def scan_windows() -> list[dict]:
    if not shutil.which("pnputil"):
        return []
    text = subprocess.run(["pnputil", "/enum-devices", "/connected"], capture_output=True, text=True,
                          check=False).stdout
    out, current = [], {}
    for line in text.splitlines() + [""]:
        if not line.strip():
            inst = current.get("Instance ID", "")
            m = re.search(r"VID_([0-9A-Fa-f]{4})&PID_([0-9A-Fa-f]{4})(?:\\([^\\&]+))?", inst)
            if m and inst.upper().startswith("USB\\"):
                out.append({"vendor_id": int(m.group(1), 16), "product_id": int(m.group(2), 16),
                            "manufacturer": current.get("Manufacturer", ""),
                            "product": current.get("Device Description", ""),
                            "serial": (m.group(3) or "") if "&" not in (m.group(3) or "") else "",
                            "bus_path": inst, "interface_classes": []})
            current = {}
            continue
        key, _, value = line.partition(":")
        current[key.strip()] = value.strip()
    return out


def is_noise(dev: dict) -> bool:
    """Class 0 (per interface) says nothing and HID may ride along a headset, so both are set aside;
    what remains has to be non-empty and made only of classes that rule out lighting."""
    classes = set(dev.get("interface_classes", [])) - {0x00, 0x03}
    return bool(classes) and classes <= NOISE_CLASSES


def scan_host_usb() -> list[dict]:
    system = platform.system()
    if system == "Linux":
        devices = scan_linux()
    elif system == "Darwin":
        devices = scan_macos()
    elif system == "Windows":
        devices = scan_windows()
    else:
        devices = []
    return [d for d in devices
            if d["vendor_id"] not in ROOT_HUB_VENDORS and USB_HUB_CLASS not in d.get("interface_classes", [])]


def host_platform() -> str:
    return {"Linux": "Linux", "Darwin": "macOS", "Windows": "Windows"}.get(platform.system(), platform.system())


# ── fallback join ────────────────────────────────────────────────────────────
def vid_pid_of(layout_device_id: str) -> tuple[int, int] | None:
    """Native layout ids look like `nollie:3061:4714:<serial>`; bridged ones do not carry VID:PID."""
    parts = layout_device_id.split(":")
    if len(parts) >= 3 and not layout_device_id.startswith(BRIDGE_PREFIX):
        try:
            return int(parts[1], 16), int(parts[2], 16)
        except ValueError:
            return None
    return None


def serial_of(layout_device_id: str) -> str:
    if layout_device_id.startswith(BRIDGE_PREFIX):
        m = re.search(r":serial:(.+)$", layout_device_id)
        return m.group(1).strip().lower() if m else ""
    parts = layout_device_id.split(":")
    return parts[3].strip().lower() if len(parts) >= 4 else ""


def is_openrgb_device(dev: dict) -> bool:
    """Bridge rows are OpenRGB routes; other drivers also report transport `bridge` (ROLI over BLE)."""
    origin = dev.get("origin", {}) or {}
    return ("openrgb" in (origin.get("driver_id"), origin.get("backend_id"))
            or dev.get("layout_device_id", "").startswith(BRIDGE_PREFIX))


def fallback_rows(daemon: Daemon, host_scan: bool, show_all: bool = False) -> tuple[list[dict], str]:
    devices = daemon.list_all("/devices") or []
    drivers = items_of(daemon.get("/drivers") or {"items": []})
    protocols: dict[tuple[int, int], tuple[str, bool]] = {}
    for drv in drivers:
        enabled = bool(drv.get("enabled", drv.get("descriptor", {}).get("default_enabled", True)))
        for proto in drv.get("protocols", []) or []:
            vid, pid = proto.get("vendor_id"), proto.get("product_id")
            if isinstance(vid, int) and isinstance(pid, int):
                protocols[(vid, pid)] = (proto.get("driver_id") or drv.get("descriptor", {}).get("id", "?"), enabled)

    rows: list[dict] = []
    by_serial: dict[str, dict] = {}
    for dev in devices:
        lid = dev.get("layout_device_id", "")
        bridged = is_openrgb_device(dev)
        bridge = dev.get("bridge") or {}
        row = {"identity": lid, "name": dev.get("name", ""), "native": None, "bridge": None, "unclaimed": False,
               "active": "none"}
        if bridged:
            row["bridge"] = {"device_id": dev["id"], "output_enabled": bridge.get("output_enabled", True),
                             "disabled_reason": bridge.get("disabled_reason")}
            row["active"] = "bridge" if row["bridge"]["output_enabled"] else "none"
        else:
            row["native"] = {"device_id": dev["id"], "driver_id": dev.get("origin", {}).get("driver_id", "?"),
                             "state": dev.get("status", dev.get("state", "?"))}
            row["active"] = "native" if row["native"]["state"] == "connected" else "none"
        serial = serial_of(lid)
        if serial and serial in by_serial:
            other = by_serial[serial]
            other["native"] = other["native"] or row["native"]
            other["bridge"] = other["bridge"] or row["bridge"]
            other["identity"] = f"{other['identity']} + {lid}"
            other["active"] = "conflict" if (other["native"] and other["bridge"]
                                             and other["bridge"]["output_enabled"]
                                             and other["native"]["state"] == "connected") else other["active"]
            continue
        if serial:
            by_serial[serial] = row
        rows.append(row)

    unclaimed_doc = daemon.get("/devices/unclaimed")
    unclaimed: list[dict]
    if unclaimed_doc is not None and not is_device_not_found(unclaimed_doc):
        unclaimed = items_of(unclaimed_doc)
        source = "daemon devices + unclaimed store"
    elif host_scan:
        known = {vid_pid_of(d.get("layout_device_id", "")) for d in devices}
        unclaimed = []
        for usb in scan_host_usb():
            key = (usb["vendor_id"], usb["product_id"])
            if key in known:
                continue
            driver, enabled = protocols.get(key, (None, True))
            if driver and enabled:
                # a native driver knows this VID:PID and is on, yet the daemon never adopted
                # the device: almost always permissions (udev, hidraw) or a claim by another app
                rows.append({"identity": f"{key[0]:04x}:{key[1]:04x}" + (f":{usb['serial']}" if usb.get("serial") else ""),
                             "name": " ".join(x for x in (usb.get("manufacturer"), usb.get("product")) if x),
                             "native": None, "bridge": None, "unclaimed": False, "active": "none",
                             "not_adopted_by": driver, "bus_path": usb.get("bus_path"),
                             "interface_classes": usb.get("interface_classes", [])})
                continue
            usb["claimable_by"] = driver if driver and not enabled else None
            unclaimed.append(usb)
        source = f"daemon devices + host USB scan ({host_platform()})"
    else:
        unclaimed, source = [], "daemon devices only (host scan disabled)"

    for u in unclaimed:
        if not show_all and is_noise(u):
            continue
        ident = f"{u['vendor_id']:04x}:{u['product_id']:04x}"
        if u.get("serial"):
            ident += f":{u['serial']}"
        serial = (u.get("serial") or "").strip().lower()
        twin = by_serial.get(serial) if serial else None
        if twin is not None:
            # same silicon the bridge (or a native device) already reports; one row, both facts
            twin["unclaimed"] = True
            twin["claimable_by"] = u.get("claimable_by")
            twin["identity"] = f"{twin['identity']} + {ident}"
            continue
        rows.append({"identity": ident, "name": " ".join(x for x in (u.get("manufacturer"), u.get("product")) if x),
                     "native": None, "bridge": None, "unclaimed": True, "active": "none",
                     "claimable_by": u.get("claimable_by"), "bus_path": u.get("bus_path"),
                     "interface_classes": u.get("interface_classes", [])})
    return rows, source


def is_device_not_found(doc) -> bool:
    """A pre-Spec-81 daemon answers /devices/coverage and /devices/unclaimed as /devices/{id} misses."""
    return isinstance(doc, dict) and (doc.get("error") or {}).get("code") in ("device_not_found", "route_not_found")


# ── report ───────────────────────────────────────────────────────────────────
def describe(row: dict) -> str:
    bits = []
    if row.get("native"):
        n = row["native"]
        bits.append(f"native {n.get('driver_id', '?')} [{n.get('state', '?')}]")
    if row.get("bridge"):
        b = row["bridge"]
        state = "output on" if b.get("output_enabled", True) else f"output OFF: {b.get('disabled_reason') or 'no reason given'}"
        bits.append(f"bridge [{state}]")
    if row.get("unclaimed"):
        hint = f"driver '{row['claimable_by']}' knows it but is disabled" if row.get("claimable_by") else "no native protocol"
        bits.append(f"unclaimed ({hint})")
    if row.get("not_adopted_by"):
        bits.append(f"known to driver '{row['not_adopted_by']}', not adopted (check permissions/udev, other software holding it)")
    return "; ".join(bits) or "no owner"


def print_report(rows: list[dict], source: str) -> None:
    groups = {"native": [], "bridge": [], "conflict": [], "unclaimed": [], "not_adopted": [], "none": []}
    for row in rows:
        owned = row.get("native") or row.get("bridge")
        if row.get("not_adopted_by"):
            key = "not_adopted"
        elif row.get("unclaimed") and not owned:
            key = "unclaimed"
        else:
            key = row.get("active", "none")
        groups.setdefault(key, []).append(row)
    titles = [("native", "Native (Hypercolor drives these)"), ("bridge", "OpenRGB bridge"),
              ("conflict", "Conflict (both stacks see the device; native wins, bridge output is off)"),
              ("unclaimed", "Unclaimed (nothing drives these yet)"),
              ("not_adopted", "Known to a native driver, not adopted (permissions, udev, or another app holds it)"),
              ("none", "Known but not driving")]
    print(f"coverage source: {source}")
    for key, title in titles:
        group = groups.get(key) or []
        if not group:
            continue
        print(f"\n{title} ({len(group)})")
        for row in sorted(group, key=lambda r: (r.get("name") or "", r["identity"])):
            name = row.get("name") or "(unnamed)"
            print(f"  {name:40.40} {row['identity']:60.60} {describe(row)}")
    if groups.get("unclaimed"):
        print("\nnext: for each unclaimed device decide native (enable the driver), bridge (OpenRGB covers it),"
              " or request support: scripts/request_support.py --vid-pid VVVV:PPPP")
    if groups.get("conflict"):
        print("\nconflict rows: hand a device to the bridge by disabling it natively"
              " (PUT /devices/{id} {\"enabled\": false}); native never yields on its own")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--base", default=DEFAULT_BASE)
    ap.add_argument("--json", action="store_true", help="print the rows as JSON instead of the grouped report")
    ap.add_argument("--all", action="store_true",
                    help="in fallback mode, also list USB devices whose classes rule out RGB (audio, storage, bluetooth)")
    ap.add_argument("--no-host-scan", action="store_true",
                    help="in fallback mode, skip lsusb/system_profiler/pnputil and report daemon devices only")
    args = ap.parse_args()
    daemon = Daemon(args.base)

    doc = daemon.get("/devices/coverage")
    if doc is not None and not is_device_not_found(doc):
        rows, source = items_of(doc), "GET /devices/coverage"
    else:
        rows, source = fallback_rows(daemon, host_scan=not args.no_host_scan, show_all=args.all)

    if args.json:
        json.dump({"source": source, "platform": host_platform(), "rows": rows}, sys.stdout, indent=1)
        print()
    else:
        print_report(rows, source)
    return 0


if __name__ == "__main__":
    sys.exit(main())
