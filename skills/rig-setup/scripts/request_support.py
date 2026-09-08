#!/usr/bin/env python3
"""Turn an unclaimed device into a Hypercolor device-support request.

    request_support.py --vid-pid 1532:029D [--vendor Razer] [--model "Blade 15"] [--file]
    request_support.py --vendor X --model Y --vid-pid VVVV:PPPP --platform Linux --existing-support "..."

Builds the prefilled GitHub issue-form URL (template device-support.yml) and prints
it. With --file and a logged-in `gh`, files the issue directly with the form fields
rendered as sections and the `hardware,device-support` labels. Before either, it
searches open and closed issues for the VID:PID and points at a match instead of
filing a duplicate (--force files anyway).

Vendor and model default from the daemon's GET /devices/unclaimed entry for that
VID:PID, then from the host USB inventory (coverage.py's scanner), so on most rigs
`--vid-pid` is the only flag you need. The form's acknowledgement checkbox cannot be
prefilled through the URL; the person filing ticks it in the browser.

Every mode, URL and --json included, runs a read-only `gh issue list --search` for the
VID:PID when gh is installed and logged in, so duplicates surface before anything is
filed. Nothing is written without --file.

Pure standard library, Python 3.11+.
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
from coverage import Daemon, is_device_not_found, items_of, scan_host_usb  # noqa: E402

DEFAULT_BASE = "http://localhost:9420/api/v1"
REPO = "hyperb1iss/hypercolor"
FORM_URL = f"https://github.com/{REPO}/issues/new"
LABELS = "hardware,device-support"
DEFAULT_EXISTING = ("Not driven by Hypercolor natively and not offered by the OpenRGB bridge at the time "
                    "of filing (checked with skills/rig-setup/scripts/coverage.py).")


def parse_vid_pid(text: str) -> tuple[int, int]:
    try:
        vid, pid = text.lower().replace("0x", "").split(":")
        return int(vid, 16), int(pid, 16)
    except ValueError as err:
        raise SystemExit(f"--vid-pid wants VVVV:PPPP hex, got '{text}'") from err


def host_platform() -> str:
    return {"Linux": "Linux", "Darwin": "macOS", "Windows": "Windows"}.get(platform.system(), "Linux")


def lookup_device(base: str, vid: int, pid: int) -> tuple[dict, str]:
    """Manufacturer/product/serial for the VID:PID and where they came from: daemon, host, or flags."""
    try:
        doc = Daemon(base).get("/devices/unclaimed")
    except SystemExit:
        doc = None
    candidates = items_of(doc) if doc is not None and not is_device_not_found(doc) else []
    source = "daemon"
    if not candidates:
        source = "host"
        try:
            candidates = scan_host_usb()
        except Exception:  # noqa: BLE001 - a broken host scanner must not block filing
            candidates = []
    for dev in candidates:
        if dev.get("vendor_id") == vid and dev.get("product_id") == pid:
            return dev, source
    return {}, "flags"


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


def build_url(fields: dict[str, str]) -> str:
    params = {"template": "device-support.yml", "title": fields["title"], "vendor": fields["vendor"],
              "model": fields["model"], "vid-pid": fields["vid_pid"], "platform": fields["platform"],
              "existing-support": fields["existing_support"]}
    return FORM_URL + "?" + urllib.parse.urlencode(params, quote_via=urllib.parse.quote)


def build_body(fields: dict[str, str]) -> str:
    sections = [("Vendor", fields["vendor"]), ("Device model", fields["model"]), ("USB VID:PID", fields["vid_pid"]),
                ("OS you're on", fields["platform"]), ("Existing support", fields["existing_support"]),
                ("Protocol notes or captures", fields.get("notes") or "None yet."),
                ("Can you test a driver on this device?", fields.get("willing", "Sometimes, when I have time"))]
    body = "\n\n".join(f"### {label}\n\n{value}" for label, value in sections)
    return body + "\n\n_Filed with skills/rig-setup/scripts/request_support.py after a coverage check._\n"


def file_issue(fields: dict[str, str]) -> str:
    proc = subprocess.run(["gh", "issue", "create", "--repo", REPO, "--title", fields["title"],
                           "--body", build_body(fields), "--label", LABELS],
                          capture_output=True, text=True, check=False)
    if proc.returncode != 0:
        raise SystemExit(f"gh issue create failed: {proc.stderr.strip()[:400]}")
    return proc.stdout.strip().splitlines()[-1]


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--vid-pid", required=True, help="USB VID:PID in hex, e.g. 1532:029D")
    ap.add_argument("--vendor", help="vendor name (default: from the unclaimed entry or host USB)")
    ap.add_argument("--model", help="device model (default: the USB product string)")
    ap.add_argument("--platform", choices=["Linux", "macOS", "Windows"], default=host_platform())
    ap.add_argument("--existing-support", default=DEFAULT_EXISTING,
                    help="what drives it today (OpenRGB, SignalRGB, the vendor app), with links if you have them")
    ap.add_argument("--notes", default="", help="protocol notes or capture pointers for the body (gh filing only)")
    ap.add_argument("--willing", default="Sometimes, when I have time",
                    help="testing availability, one of the form's dropdown options (gh filing only)")
    ap.add_argument("--base", default=DEFAULT_BASE)
    ap.add_argument("--file", action="store_true", help="file with gh instead of printing the URL")
    ap.add_argument("--force", action="store_true", help="file even when an issue already mentions this VID:PID")
    ap.add_argument("--json", action="store_true", help="print fields, URL, and duplicate matches as JSON")
    args = ap.parse_args()

    vid, pid = parse_vid_pid(args.vid_pid)
    vid_pid = f"{vid:04X}:{pid:04X}"
    found, source = lookup_device(args.base, vid, pid)
    vendor = args.vendor or found.get("manufacturer") or f"Unknown vendor {vid:04X}"
    model = args.model or found.get("product") or f"Unknown device {pid:04X}"
    fields = {"vendor": vendor, "model": model, "vid_pid": vid_pid, "platform": args.platform,
              "existing_support": args.existing_support, "notes": args.notes, "willing": args.willing,
              "title": f"[device] {vendor} {model} ({vid_pid})"}
    url = build_url(fields)
    dupes = existing_issues(vid_pid)

    if args.json:
        json.dump({"fields": fields, "url": url, "existing_issues": dupes, "gh_ready": gh_ready(),
                   "source": source}, sys.stdout, indent=1)
        print()
        return 0

    if dupes and not args.force:
        print(f"an issue already mentions {vid_pid}; add your details there instead of filing twice:")
        for issue in dupes:
            print(f"  #{issue['number']} [{issue['state']}] {issue['title']}\n     {issue['url']}")
        return 0

    if args.file:
        if not gh_ready():
            print("gh is missing or not logged in (`gh auth login`); open this URL instead:\n" + url)
            return 2
        print("filed: " + file_issue(fields))
        return 0

    print(f"prefilled device-support form for {vendor} {model} ({vid_pid}, {args.platform}):\n{url}")
    print("\ntick the 'Before submitting' checkbox in the browser; the form cannot prefill it."
          "\nadd captures or protocol notes if you have them, then submit."
          "\n(--file files it with gh instead, when gh is logged in)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
