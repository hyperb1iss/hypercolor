"""Prepare a Hypercolor device-support form without submitting an issue.

    request_support.py --vid-pid 1532:029D [--product "Razer Blade 15"]
    request_support.py --vid-pid VVVV:PPPP --platform Windows --json

Detected USB facts and the daemon version are kept separate from the owner's
product identity and lighting answers. --product accepts a retail identity the
owner supplied; USB descriptors never fill that answer.

A read-only gh search lists existing requests when gh is installed and logged in.
Open the generated URL to complete the product and lighting questions. Nothing is
posted by this script. Pure standard library, Python 3.11+.
"""
from __future__ import annotations

import argparse
import json
import platform
import shutil
import subprocess
import sys
import urllib.parse
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from coverage import Daemon, is_device_not_found, items_of, scan_host_usb

DEFAULT_BASE = "http://localhost:9420/api/v1"
REPO = "hyperb1iss/hypercolor"
FORM_URL = f"https://github.com/{REPO}/issues/new"


def parse_vid_pid(text: str) -> tuple[int, int]:
    try:
        vid, pid = text.lower().replace("0x", "").split(":")
        pair = int(vid, 16), int(pid, 16)
        if not all(0 <= value <= 0xFFFF for value in pair):
            raise ValueError("USB IDs must fit in 16 bits")
        return pair
    except ValueError as err:
        raise SystemExit(f"--vid-pid wants VVVV:PPPP hex, got '{text}'") from err


def host_platform() -> str:
    return {"Linux": "Linux", "Darwin": "macOS", "Windows": "Windows"}.get(platform.system(), "")


def lookup_device(base: str, vid: int, pid: int) -> tuple[dict, str]:
    """Find detected USB facts in the daemon inventory, then the host inventory."""
    try:
        doc = Daemon(base).get("/devices/unclaimed")
    except SystemExit:
        doc = None
    candidates = items_of(doc) if doc is not None and not is_device_not_found(doc) else []
    for dev in candidates:
        if dev.get("vendor_id") == vid and dev.get("product_id") == pid:
            return dev, "daemon"
    try:
        candidates = scan_host_usb()
    except Exception:  # noqa: BLE001 - inventory failure must not block the form
        candidates = []
    for dev in candidates:
        if dev.get("vendor_id") == vid and dev.get("product_id") == pid:
            return dev, "host"
    return {}, "not found"


def daemon_version(base: str) -> str | None:
    """Read the running daemon version, leaving it unanswered when unavailable."""
    try:
        doc = Daemon(base).get("/system")
    except (SystemExit, OSError, json.JSONDecodeError):
        return None
    if not isinstance(doc, dict):
        return None
    data = doc.get("data")
    identity = data.get("identity") if isinstance(data, dict) else None
    version = identity.get("version") if isinstance(identity, dict) else None
    return version.strip() if isinstance(version, str) and version.strip() else None


def gh_ready() -> bool:
    if not shutil.which("gh"):
        return False
    return subprocess.run(["gh", "auth", "status"], capture_output=True, text=True, check=False).returncode == 0


def existing_issues(vid_pid: str) -> list[dict]:
    """Open and closed issues that mention the VID:PID, via gh search (read-only)."""
    if not gh_ready():
        return []
    proc = subprocess.run(["gh", "issue", "list", "--repo", REPO, "--state", "all", "--limit", "10",
                           "--search", vid_pid, "--json", "number,title,url,state"],
                          capture_output=True, text=True, check=False)
    if proc.returncode != 0:
        return []
    try:
        return json.loads(proc.stdout or "[]")
    except json.JSONDecodeError:
        return []


def detected_details(device: dict, source: str) -> str:
    """Public detection facts, excluding serial numbers and host paths."""
    lines = [f"Inventory source: {source}",
             f"USB vendor label: {device.get('manufacturer') or 'Not reported'}",
             f"USB product string: {device.get('product') or 'Not reported'}"]
    classes = device.get("interface_classes") or []
    lines.append("USB interface classes: " + (", ".join(f"0x{value:02X}" for value in classes) or "Not reported"))
    driver = device.get("claimable_by")
    if driver:
        lines.append(f"Native driver: matched disabled driver {driver}.")
    elif source == "daemon":
        lines.append("Native driver: no match reported in the unclaimed inventory.")
    else:
        lines.append("Native driver: not checked.")
    lines.extend(["OpenRGB support: not checked.",
                  "", "USB detection does not establish lighting capability or the retail brand/model."])
    return "\n".join(lines)


def build_url(fields: dict[str, str]) -> str:
    params = {"template": "device-support.yml", **fields}
    return FORM_URL + "?" + urllib.parse.urlencode(params, quote_via=urllib.parse.quote)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--vid-pid", required=True, help="USB VID:PID in hex, e.g. 1532:029D")
    ap.add_argument("--product", help="retail brand/model supplied by the owner, not a USB descriptor")
    ap.add_argument("--platform", choices=["Linux", "macOS", "Windows"], default=host_platform())
    ap.add_argument("--base", default=DEFAULT_BASE)
    ap.add_argument("--json", action="store_true", help="print fields, URL, and duplicate matches as JSON")
    args = ap.parse_args()

    vid, pid = parse_vid_pid(args.vid_pid)
    vid_pid = f"{vid:04X}:{pid:04X}"
    found, source = lookup_device(args.base, vid, pid)
    fields = {"vid-pid": vid_pid, "detected-device": detected_details(found, source)}
    if args.platform:
        fields["platform"] = args.platform
    version = daemon_version(args.base)
    if version:
        fields["version"] = version
    if args.product and args.product.strip():
        fields["product"] = args.product.strip()
    url = build_url(fields)
    dupes = existing_issues(vid_pid)

    if args.json:
        json.dump({"fields": fields, "url": url, "existing_issues": dupes, "source": source}, sys.stdout, indent=1)
        print()
        return 0

    if dupes:
        print(f"existing issues mention {vid_pid}; check whether one covers your product:")
        for issue in dupes:
            print(f"  #{issue['number']} [{issue['state']}] {issue['title']}\n     {issue['url']}")
        print()

    print(f"device-support form with detected facts for {vid_pid}:\n{url}")
    print("\ncomplete the product identity and lighting questions in the form."
          "\nif an existing issue covers your product, add your details there instead.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
